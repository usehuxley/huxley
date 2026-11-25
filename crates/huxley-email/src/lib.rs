//! Outgoing email (design doc §3: `lettre` for SMTP).
//!
//! [`Mailer`] sends plain-text + HTML messages over SMTP, records them in
//! memory (tests), or is disabled when no SMTP server is configured: callers
//! check [`Mailer::enabled`] to hide features that need email.
//!
//! Configuration: `email.smtp_url` (`HUXLEY_EMAIL__SMTP_URL[_FILE]`), e.g.
//! `smtps://user:pass@smtp.example.com` (implicit TLS, port 465),
//! `smtp://user:pass@smtp.example.com?tls=required` (STARTTLS, port 587), or
//! `smtp://localhost:1025` (plain, local relays such as Mailpit), and
//! `email.from`, e.g. `Huxley <huxley@example.com>`.

use std::sync::{Arc, Mutex};

use lettre::message::{Mailbox, MultiPart, header::ContentType};
use lettre::{AsyncSmtpTransport, AsyncTransport as _, Message, Tokio1Executor};
use secrecy::{ExposeSecret as _, SecretString};
use serde::Deserialize;

/// Email settings (`[email]` in `huxley.toml`, or `HUXLEY_EMAIL__*`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct EmailConfig {
    /// SMTP server URL. Unset: email is off, and features that need it
    /// (password reset, invites, verification) are hidden.
    pub smtp_url: Option<SecretString>,
    /// The From address, e.g. `Huxley <huxley@example.com>`.
    pub from: Option<String>,
}

/// How to reach an SMTP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpSecurity {
    /// TLS from the start (usually port 465).
    Tls,
    /// STARTTLS (usually port 587).
    StartTls,
    /// Neither: a server on your own network.
    None,
}

/// An SMTP server and the login to send with.
#[derive(Clone)]
pub struct SmtpServer {
    pub host: String,
    pub port: u16,
    pub security: SmtpSecurity,
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for SmtpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpServer")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("security", &self.security)
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EmailError {
    #[error("invalid email.smtp_url: {0}")]
    InvalidUrl(String),
    #[error("email.from is required when email.smtp_url is set, as `Name <address>`")]
    InvalidFrom,
    #[error("invalid recipient address")]
    InvalidRecipient,
    #[error("email is not configured")]
    Disabled,
    #[error("sending failed: {0}")]
    Send(String),
}

/// One message, as sent (or recorded in tests).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Email {
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
    /// Sent as a reply from a mailbox, rather than from the instance.
    pub reply: Option<Reply>,
}

/// A message sent from a mailbox, threaded into a conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The display name, e.g. "Acme Support".
    pub from_name: String,
    /// The mailbox's address (also where replies go).
    pub from_address: String,
    /// Without angle brackets.
    pub message_id: String,
    pub in_reply_to: Vec<String>,
    pub references: Vec<String>,
}

#[derive(Clone)]
enum Transport {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    Memory(Arc<Mutex<Vec<Email>>>),
    Disabled,
}

/// Sends email. Cheap to clone.
#[derive(Clone)]
pub struct Mailer {
    transport: Transport,
    from: Mailbox,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = match self.transport {
            Transport::Smtp(_) => "smtp",
            Transport::Memory(_) => "memory",
            Transport::Disabled => "disabled",
        };
        f.debug_struct("Mailer")
            .field("transport", &kind)
            .field("from", &self.from.to_string())
            .finish()
    }
}

impl Mailer {
    pub fn from_config(config: &EmailConfig) -> Result<Self, EmailError> {
        // Empty counts as unset, so deployments can leave the variable blank.
        let Some(url) = config
            .smtp_url
            .as_ref()
            .filter(|u| !u.expose_secret().trim().is_empty())
        else {
            return Ok(Self::disabled());
        };
        let from: Mailbox = config
            .from
            .as_deref()
            .ok_or(EmailError::InvalidFrom)?
            .parse()
            .map_err(|_| EmailError::InvalidFrom)?;
        let transport = AsyncSmtpTransport::<Tokio1Executor>::from_url(url.expose_secret())
            .map_err(|e| EmailError::InvalidUrl(e.to_string()))?
            .timeout(Some(std::time::Duration::from_secs(15)))
            .build();
        Ok(Self {
            transport: Transport::Smtp(transport),
            from,
        })
    }

    /// A mailer for one SMTP server with its own login (a mailbox's), in
    /// place of the instance's. Messages need a `reply` (its From).
    pub fn server(server: &SmtpServer) -> Result<Self, EmailError> {
        let builder = match server.security {
            SmtpSecurity::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&server.host),
            SmtpSecurity::StartTls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&server.host)
            }
            SmtpSecurity::None => Ok(AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(
                &server.host,
            )),
        }
        .map_err(|e| EmailError::InvalidUrl(e.to_string()))?;
        let mut builder = builder
            .port(server.port)
            .timeout(Some(std::time::Duration::from_secs(15)));
        if !server.username.is_empty() {
            builder =
                builder.credentials(lettre::transport::smtp::authentication::Credentials::new(
                    server.username.clone(),
                    server.password.clone(),
                ));
        }
        Ok(Self {
            transport: Transport::Smtp(builder.build()),
            from: "Huxley <huxley@localhost.invalid>"
                .parse()
                .expect("valid mailbox"),
        })
    }

    pub fn disabled() -> Self {
        Self {
            transport: Transport::Disabled,
            from: "Huxley <huxley@localhost.invalid>"
                .parse()
                .expect("valid mailbox"),
        }
    }

    /// A mailer that keeps messages in memory. For tests.
    pub fn memory() -> (Self, Arc<Mutex<Vec<Email>>>) {
        let store = Arc::new(Mutex::new(Vec::new()));
        let mailer = Self {
            transport: Transport::Memory(store.clone()),
            from: "Huxley <huxley@example.com>"
                .parse()
                .expect("valid mailbox"),
        };
        (mailer, store)
    }

    pub fn enabled(&self) -> bool {
        !matches!(self.transport, Transport::Disabled)
    }

    pub async fn send(&self, email: Email) -> Result<(), EmailError> {
        match &self.transport {
            Transport::Disabled => Err(EmailError::Disabled),
            Transport::Memory(store) => {
                store.lock().expect("store lock").push(email);
                Ok(())
            }
            Transport::Smtp(smtp) => {
                let to: Mailbox = email.to.parse().map_err(|_| EmailError::InvalidRecipient)?;
                let mut builder = Message::builder().to(to).subject(&email.subject);
                builder = match &email.reply {
                    None => builder.from(self.from.clone()),
                    Some(reply) => {
                        let from = Mailbox::new(
                            Some(reply.from_name.clone()),
                            reply
                                .from_address
                                .parse()
                                .map_err(|_| EmailError::InvalidFrom)?,
                        );
                        let ids = |list: &[String]| {
                            list.iter()
                                .map(|id| format!("<{id}>"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        };
                        let mut b = builder
                            .from(from.clone())
                            .reply_to(from)
                            .message_id(Some(format!("<{}>", reply.message_id)));
                        if !reply.in_reply_to.is_empty() {
                            b = b.in_reply_to(ids(&reply.in_reply_to));
                        }
                        if !reply.references.is_empty() {
                            b = b.references(ids(&reply.references));
                        }
                        b
                    }
                };
                let message = builder
                    .multipart(
                        MultiPart::alternative()
                            .singlepart(
                                lettre::message::SinglePart::builder()
                                    .header(ContentType::TEXT_PLAIN)
                                    .body(email.text),
                            )
                            .singlepart(
                                lettre::message::SinglePart::builder()
                                    .header(ContentType::TEXT_HTML)
                                    .body(email.html),
                            ),
                    )
                    .map_err(|e| EmailError::Send(e.to_string()))?;
                smtp.send(message)
                    .await
                    .map(|_| ())
                    .map_err(|e| EmailError::Send(e.to_string()))
            }
        }
    }

    /// Sends in the background and logs failures. For messages where the
    /// caller must respond the same way whether or not one was sent.
    pub fn send_later(&self, email: Email) {
        let mailer = self.clone();
        tokio::spawn(async move {
            let to_domain = email.to.rsplit_once('@').map(|(_, d)| d.to_owned());
            if let Err(error) = mailer.send(email).await {
                tracing::error!(%error, to_domain, "failed to send email");
            }
        });
    }
}

/// Escapes text for HTML element content and attribute values.
pub fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

/// A simple transactional email with one call-to-action link.
pub fn action_email(
    to: &str,
    subject: &str,
    greeting: &str,
    body: &str,
    button: &str,
    link: &str,
    footer: &str,
) -> Email {
    let text = format!("{greeting}\n\n{body}\n\n{button}: {link}\n\n{footer}\n");
    let html = format!(
        "<!doctype html><html><body style=\"font-family:system-ui,sans-serif;color:#1f2328;line-height:1.5\">\
         <p>{greeting}</p><p>{body}</p>\
         <p><a href=\"{link}\" style=\"display:inline-block;padding:10px 16px;background:#4f46e5;color:#fff;\
         border-radius:6px;text-decoration:none\">{button}</a></p>\
         <p style=\"color:#57606a;font-size:13px\">{footer}</p></body></html>",
        greeting = escape_html(greeting),
        body = escape_html(body),
        button = escape_html(button),
        link = escape_html(link),
        footer = escape_html(footer),
    );
    Email {
        to: to.to_owned(),
        subject: subject.to_owned(),
        text,
        html,
        reply: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            escape_html("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn action_emails_escape_user_text() {
        let e = action_email(
            "a@example.com",
            "S",
            "Hi <b>Eve</b>",
            "Body",
            "Go",
            "https://x/?a=1&b=2",
            "F",
        );
        assert!(e.html.contains("Hi &lt;b&gt;Eve&lt;/b&gt;"));
        assert!(e.html.contains("https://x/?a=1&amp;b=2"));
        assert!(e.text.contains("https://x/?a=1&b=2"));
    }

    #[test]
    fn config_validation() {
        assert!(
            !Mailer::from_config(&EmailConfig::default())
                .unwrap()
                .enabled()
        );
        let no_from = EmailConfig {
            smtp_url: Some(SecretString::from("smtp://localhost:1025")),
            from: None,
        };
        assert!(matches!(
            Mailer::from_config(&no_from),
            Err(EmailError::InvalidFrom)
        ));
        let bad_url = EmailConfig {
            smtp_url: Some(SecretString::from("nope://x")),
            from: Some("Huxley <h@example.com>".into()),
        };
        assert!(matches!(
            Mailer::from_config(&bad_url),
            Err(EmailError::InvalidUrl(_))
        ));
    }

    #[tokio::test]
    async fn memory_mailer_records() {
        let (mailer, store) = Mailer::memory();
        mailer
            .send(action_email(
                "a@example.com",
                "S",
                "G",
                "B",
                "Go",
                "https://x",
                "F",
            ))
            .await
            .unwrap();
        assert_eq!(store.lock().unwrap().len(), 1);
        assert!(matches!(
            Mailer::disabled()
                .send(action_email(
                    "a@example.com",
                    "S",
                    "G",
                    "B",
                    "Go",
                    "https://x",
                    "F"
                ))
                .await,
            Err(EmailError::Disabled)
        ));
    }
}
