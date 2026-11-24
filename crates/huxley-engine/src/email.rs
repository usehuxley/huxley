//! The `core.email` step: sends an email to any address through the
//! instance's mail server (`email.smtp_url`). Each recipient gets their own
//! copy, so they don't see each other.

use serde_json::{Value as Json, json};

use crate::Engine;
use crate::walk::error;

/// Most recipients one step sends to.
pub const MAX_RECIPIENTS: usize = 20;

/// A plausible single address: no display names, lists or headers.
fn address(text: &str) -> Option<&str> {
    let text = text.trim();
    let (local, domain) = text.split_once('@')?;
    let ok = !local.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && text.len() <= 254
        && !text
            .contains(|c: char| c.is_whitespace() || c.is_control() || "<>,;:\\\"()[]".contains(c))
        && text.matches('@').count() == 1;
    ok.then_some(text)
}

fn recipients(to: &Json) -> Result<Vec<String>, Json> {
    let given: Vec<&Json> = match to {
        Json::Array(items) => items.iter().collect(),
        one => vec![one],
    };
    let mut out = Vec::new();
    for item in given {
        let text = item.as_str().unwrap_or_default();
        let Some(address) = address(text) else {
            return Err(error("input", format!("{text:?} isn't an email address")));
        };
        if !out.iter().any(|a: &String| a.eq_ignore_ascii_case(address)) {
            out.push(address.to_owned());
        }
    }
    if out.is_empty() {
        return Err(error("input", "`to` needs at least one address"));
    }
    if out.len() > MAX_RECIPIENTS {
        return Err(error(
            "input",
            format!("at most {MAX_RECIPIENTS} recipients per step"),
        ));
    }
    Ok(out)
}

impl Engine {
    pub(crate) async fn send_email(&self, input: &Json) -> Result<Json, Json> {
        let Some(mailer) = self.mailer.as_ref().filter(|m| m.enabled()) else {
            return Err(error(
                "email",
                "email isn't set up on this instance (email.smtp_url)",
            ));
        };
        let to = recipients(&input["to"])?;
        let subject = input["subject"].as_str().unwrap_or_default().trim();
        if subject.is_empty() || subject.contains(['\r', '\n']) {
            return Err(error("input", "`subject` is one line of text"));
        }
        let text = input["text"].as_str().unwrap_or_default().to_owned();
        let html = input["html"].as_str().map_or_else(
            || {
                format!(
                    "<p>{}</p>",
                    huxley_email::escape_html(&text).replace('\n', "<br>")
                )
            },
            str::to_owned,
        );
        for address in &to {
            let email = huxley_email::Email {
                to: address.clone(),
                subject: subject.to_owned(),
                text: text.clone(),
                html: html.clone(),
                reply: None,
            };
            if let Err(e) = mailer.send(email).await {
                return Err(error("email", format!("sending to {address} failed: {e}")));
            }
        }
        Ok(json!({ "sent": to.len(), "to": to }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_checked() {
        assert_eq!(
            recipients(&json!(["a@example.com", "A@example.com", "b@x.io"])).unwrap(),
            vec!["a@example.com", "b@x.io"]
        );
        for bad in [
            json!("no-at"),
            json!("Ada <a@example.com>"),
            json!("a@example.com, b@example.com"),
            json!("a@example.com\r\nBcc: x@evil.com"),
            json!("a@localhost"),
            json!([]),
            json!(null),
        ] {
            assert!(recipients(&bad).is_err(), "{bad}");
        }
        let many: Vec<String> = (0..21).map(|i| format!("u{i}@example.com")).collect();
        assert!(recipients(&json!(many)).is_err());
    }
}
