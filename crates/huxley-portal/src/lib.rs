//! The client portal's realm (ADR-040): portal accounts for client
//! contacts, their sessions, and signing in by emailed link or password.
//!
//! Portal accounts aren't users. Nothing here is read by staff
//! authentication, and staff sessions never authenticate portal requests.

pub mod accounts;
pub mod branding;
pub mod mfa;
pub mod signin;
pub mod sso;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

/// The module's key in the registry.
pub const MODULE: &str = "portal";

#[derive(Debug, thiserror::Error)]
pub enum PortalError {
    #[error("not found")]
    NotFound,
    #[error("{field}: {message}")]
    Invalid { field: String, message: String },
    #[error("{0}")]
    Conflict(String),
    #[error("it was changed by someone else; reload and try again")]
    VersionConflict,
    /// Wrong email or password, or the link is used, expired or unknown.
    #[error("that didn't work")]
    BadCredentials,
    /// A wrong two-factor code (or the challenge is used up or expired).
    #[error("that code didn't work")]
    BadCode,
    /// Too many wrong passwords: try again after this many seconds.
    #[error("too many attempts; try again in {0} seconds")]
    Locked(i64),
    /// Signing in with an identity provider failed (a short reason, e.g.
    /// `no_account`, `domain`, `provider`).
    #[error("sign-in failed: {0}")]
    SignIn(&'static str),
    #[error(transparent)]
    Db(#[from] huxley_db::DbError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl PortalError {
    pub(crate) fn invalid(field: &str, message: impl Into<String>) -> Self {
        Self::Invalid {
            field: field.to_owned(),
            message: message.into(),
        }
    }
}

pub type PortalResult<T> = Result<T, PortalError>;

/// A portal account's role at its company.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[schema(as = PortalRole)]
pub enum Role {
    /// Their own tickets; the service catalog and knowledge base.
    ClientUser,
    /// As a client user, and approves what's sent to them.
    ClientApprover,
    /// Everyone's tickets at their company, their assets, and their
    /// colleagues' portal access.
    ClientAdmin,
}

/// What a portal account may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perm {
    /// See, raise and reply to their own tickets.
    TicketsOwn,
    /// See and reply to every ticket of their company.
    TicketsCompany,
    /// Decide approvals sent to them.
    Approve,
    /// See their company's assets.
    Assets,
    /// Invite colleagues and change their portal access.
    ManageContacts,
    /// See the dashboards published to the portal, with the company's numbers.
    Dashboards,
    /// See the company's projects: their milestones and progress.
    Projects,
    /// See and answer every quote sent to the company (anyone sees and
    /// answers quotes addressed to them).
    Quotes,
    /// See the company's invoices, what's owed, and pay them.
    Invoices,
}

impl Role {
    pub fn can(self, perm: Perm) -> bool {
        match perm {
            Perm::TicketsOwn => true,
            Perm::Approve | Perm::Projects | Perm::Quotes | Perm::Invoices => {
                matches!(self, Self::ClientApprover | Self::ClientAdmin)
            }
            Perm::TicketsCompany | Perm::Assets | Perm::ManageContacts | Perm::Dashboards => {
                self == Self::ClientAdmin
            }
        }
    }

    /// The portal permission strings it holds (for the portal app).
    pub fn permissions(self) -> Vec<&'static str> {
        [
            (Perm::TicketsOwn, "portal.ticket:read:own"),
            (Perm::TicketsCompany, "portal.ticket:read:company"),
            (Perm::Approve, "portal.approval:decide"),
            (Perm::Assets, "portal.asset:read"),
            (Perm::ManageContacts, "portal.contacts:manage"),
            (Perm::Dashboards, "portal.dashboard:read"),
            (Perm::Projects, "portal.project:read"),
            (Perm::Quotes, "portal.quote:read"),
            (Perm::Invoices, "portal.invoice:read"),
        ]
        .into_iter()
        .filter(|(p, _)| self.can(*p))
        .map(|(_, name)| name)
        .collect()
    }
}

/// Who is signed in to the portal.
#[derive(Debug, Clone, Serialize, ToSchema, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub struct PortalPrincipal {
    pub account_id: Uuid,
    pub contact_id: Uuid,
    /// Their company.
    pub org_id: Uuid,
    pub org_name: String,
    pub name: String,
    pub email: String,
    pub role: Role,
    #[serde(skip)]
    pub session_id: Uuid,
    /// Their company requires two-factor sign-in and they haven't set it
    /// up: only the setup is open to them.
    pub mfa_setup_required: bool,
}

impl PortalPrincipal {
    pub fn can(&self, perm: Perm) -> bool {
        self.role.can(perm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_grant_what_they_say() {
        assert!(Role::ClientUser.can(Perm::TicketsOwn));
        assert!(!Role::ClientUser.can(Perm::TicketsCompany));
        assert!(!Role::ClientUser.can(Perm::Approve));
        assert!(Role::ClientApprover.can(Perm::Approve));
        assert!(!Role::ClientApprover.can(Perm::ManageContacts));
        assert_eq!(
            Role::ClientAdmin.permissions(),
            [
                "portal.ticket:read:own",
                "portal.ticket:read:company",
                "portal.approval:decide",
                "portal.asset:read",
                "portal.contacts:manage",
                "portal.dashboard:read",
                "portal.project:read",
                "portal.quote:read",
                "portal.invoice:read"
            ]
        );
    }
}
