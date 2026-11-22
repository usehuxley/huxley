//! Who is acting, and through what channel. Carried into every mutation so it
//! can be written to the audit log.

use std::net::IpAddr;

use serde::Serialize;
use uuid::Uuid;

/// The principal performing an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "type", content = "id", rename_all = "lowercase")]
pub enum Actor {
    User(Uuid),
    /// Huxley itself: migrations, the scheduler, bootstrap.
    System,
    /// An unauthenticated caller, e.g. a failed login.
    Anonymous,
}

impl Actor {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::User(_) => "user",
            Self::System => "system",
            Self::Anonymous => "anonymous",
        }
    }

    pub fn id(&self) -> Option<Uuid> {
        match self {
            Self::User(id) => Some(*id),
            Self::System | Self::Anonymous => None,
        }
    }
}

/// The channel an action came through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Via {
    Ui,
    Api,
    Cli,
    Mcp,
    Workflow,
    System,
}

impl Via {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ui => "ui",
            Self::Api => "api",
            Self::Cli => "cli",
            Self::Mcp => "mcp",
            Self::Workflow => "workflow",
            Self::System => "system",
        }
    }
}

/// Request metadata attached to audit records.
#[derive(Debug, Clone)]
pub struct RequestContext {
    pub actor: Actor,
    pub via: Via,
    pub ip: Option<IpAddr>,
    pub user_agent: Option<String>,
    pub request_id: Option<String>,
}

/// User agents longer than this are truncated before storage.
pub const MAX_USER_AGENT_LEN: usize = 512;

impl RequestContext {
    /// Context for work Huxley does on its own behalf.
    pub fn system(via: Via) -> Self {
        Self {
            actor: Actor::System,
            via,
            ip: None,
            user_agent: None,
            request_id: None,
        }
    }

    #[must_use]
    pub fn with_actor(mut self, actor: Actor) -> Self {
        self.actor = actor;
        self
    }

    /// The user agent, truncated on a character boundary.
    pub fn user_agent_truncated(&self) -> Option<&str> {
        self.user_agent.as_deref().map(|ua| {
            if ua.len() <= MAX_USER_AGENT_LEN {
                ua
            } else {
                let mut end = MAX_USER_AGENT_LEN;
                while !ua.is_char_boundary(end) {
                    end -= 1;
                }
                &ua[..end]
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actor_accessors() {
        let id = Uuid::now_v7();
        assert_eq!(Actor::User(id).kind(), "user");
        assert_eq!(Actor::User(id).id(), Some(id));
        assert_eq!(Actor::System.id(), None);
    }

    #[test]
    fn user_agent_truncates_on_char_boundary() {
        let mut ctx = RequestContext::system(Via::Api);
        ctx.user_agent = Some("é".repeat(400)); // 800 bytes
        let ua = ctx.user_agent_truncated().unwrap();
        assert!(ua.len() <= MAX_USER_AGENT_LEN);
        assert!(ua.chars().all(|c| c == 'é'));
    }
}
