//! The authorized-user extractor and error mapping for domain errors.

use axum::extract::FromRequestParts;
use http::StatusCode;
use http::request::Parts;
use huxley_auth::AuthError;
use huxley_authz::{AuthzError, Principal, permissions};
use huxley_db::{OrgScope, begin_scoped};
use huxley_orgs::OrgError;
use uuid::Uuid;

use crate::session::CurrentUser;
use crate::{ApiError, ApiResult, AppState};

/// A signed-in user and their grants. Handlers check permissions through it.
///
/// Extracting it also enforces the MFA policy: a user whom policy requires
/// to have a second factor, and who has none, gets `403 mfa-setup-required`
/// until they set one up (those routes take [`CurrentUser`] instead).
///
/// Accepts a session cookie or `Authorization: Bearer <API token>`. With a
/// token, the principal's grants are narrowed to the token's scopes.
#[derive(Debug, Clone)]
pub struct Authed {
    pub user: CurrentUser,
    pub principal: Principal,
}

impl FromRequestParts<AppState> for Authed {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        let user = match bearer_token(&parts.headers).map(str::to_owned) {
            Some(secret) => CurrentUser::from_api_token(parts, state, &secret).await?,
            None => CurrentUser::from_request_parts(parts, state).await?,
        };
        let authed = Self::load(user, state).await?;
        if authed.mfa_setup_required() {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "mfa-setup-required",
                "Two-factor authentication is required",
            )
            .with_detail(
                "Your organization requires two-factor authentication. Add a passkey or an \
                 authenticator app from your profile to continue.",
            ));
        }
        Ok(authed)
    }
}

/// The secret from `Authorization: Bearer …`, if present.
pub fn bearer_token(headers: &http::HeaderMap) -> Option<&str> {
    headers
        .get(http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
        .map(str::trim)
}

impl Authed {
    /// Loads the user's grants without enforcing the MFA policy.
    pub async fn load(user: CurrentUser, state: &AppState) -> ApiResult<Self> {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        let principal = huxley_authz::load_principal(&mut tx, user.auth.user.id)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        tx.commit().await.map_err(|e| ApiError::internal(&e))?;
        let mut principal = principal;
        if let Some(grant) = &user.auth.token {
            principal.restrict_to(&grant.scopes);
        }
        Ok(Self { user, principal })
    }

    /// Policy requires a second factor and the user has none. Service
    /// accounts are exempt: they have no interactive sign-in to protect.
    pub fn mfa_setup_required(&self) -> bool {
        self.user.auth.user.kind == huxley_auth::UserKind::Human
            && self.principal.requires_mfa
            && !self.user.auth.user.mfa_enabled
            && !self.user.auth.session_mfa
    }

    /// Requires an instance-wide permission.
    pub fn require_instance(&self, permission: &str) -> ApiResult<()> {
        if self.principal.can_instance(permission) {
            Ok(())
        } else {
            Err(ApiError::forbidden())
        }
    }

    /// Requires a permission at an org. Responds 404 rather than 403 when the
    /// caller can't even see the org, so existence isn't revealed.
    pub async fn require_at(
        &self,
        state: &AppState,
        permission: &str,
        org_id: Uuid,
    ) -> ApiResult<()> {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        let path = huxley_authz::org_path(&mut tx, org_id)
            .await
            .map_err(map_authz)?;
        tx.commit().await.map_err(|e| ApiError::internal(&e))?;
        if self.principal.can_at_path(permission, &path) {
            Ok(())
        } else if self.principal.can_at_path(permissions::ORG_READ, &path) {
            Err(ApiError::forbidden())
        } else {
            Err(ApiError::not_found())
        }
    }

    /// The RLS scope for this user: every org where they hold a grant.
    pub async fn scope(&self, state: &AppState) -> ApiResult<OrgScope> {
        let mut tx = begin_scoped(&state.db, &OrgScope::System)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        let orgs = huxley_authz::visible_orgs(&mut tx, &self.principal)
            .await
            .map_err(|e| ApiError::internal(&e))?;
        tx.commit().await.map_err(|e| ApiError::internal(&e))?;
        Ok(OrgScope::Orgs(orgs))
    }
}

pub fn map_org(error: OrgError) -> ApiError {
    match error {
        OrgError::NotFound | OrgError::UnknownSetting => ApiError::not_found(),
        OrgError::NameTaken => {
            ApiError::conflict("An active sibling organization already uses this name.")
        }
        OrgError::RootExists => ApiError::conflict("The instance already has a root organization."),
        OrgError::HasActiveChildren => {
            ApiError::conflict("Archive or move its active sub-organizations first.")
        }
        OrgError::VersionConflict => ApiError::conflict(
            "The organization was changed by someone else. Reload and try again.",
        ),
        OrgError::InvalidSetting(message) => {
            ApiError::validation().with_field_error("/value", message)
        }
        OrgError::SettingLocked(by) => ApiError::conflict(format!(
            "{by} locked this setting, so it can't be changed here."
        )),
        OrgError::InvalidName(reason) => {
            ApiError::validation().with_field_error("/name", format!("Name {reason}."))
        }
        e @ (OrgError::ProviderUnderClient | OrgError::Cycle | OrgError::RootImmutable) => {
            ApiError::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid-org-change",
                "Invalid change",
            )
            .with_detail(format!(
                "{}{}",
                e.to_string()[..1].to_uppercase(),
                &e.to_string()[1..]
            ))
        }
        OrgError::Db(e) => ApiError::internal(&e),
    }
}

pub fn map_authz(error: AuthzError) -> ApiError {
    match error {
        AuthzError::Forbidden => ApiError::forbidden(),
        AuthzError::SystemRole => ApiError::conflict("System roles can't be changed or deleted."),
        AuthzError::RoleInUse(n) => ApiError::conflict(format!(
            "The role is assigned {n} time(s). Remove those assignments first."
        )),
        AuthzError::RoleKeyTaken => ApiError::conflict("A role with this key already exists."),
        AuthzError::InvalidRole(message) => {
            ApiError::validation().with_field_error("/permissions", message)
        }
        AuthzError::Escalation(missing) => ApiError::forbidden().with_detail(format!(
            "You can only grant permissions you hold yourself. Missing: {missing}."
        )),
        AuthzError::OwnerToTeam => {
            ApiError::conflict("The Owner role is granted to individual people, not teams.")
        }
        AuthzError::TeamNameTaken => ApiError::conflict("A team with this name already exists."),
        AuthzError::InvalidTeamName => ApiError::validation().with_field_error(
            "/name",
            "Enter a name of 1 to 100 characters (description up to 500).",
        ),
        AuthzError::RoleNotFound
        | AuthzError::AssignmentNotFound
        | AuthzError::OrgNotFound
        | AuthzError::TeamNotFound
        | AuthzError::UserNotFound => ApiError::not_found(),
        AuthzError::AlreadyAssigned => {
            ApiError::conflict("The user already has this role at this organization.")
        }
        AuthzError::OwnerOnly => {
            ApiError::new(StatusCode::FORBIDDEN, "owner-only", "Owner required")
                .with_detail("Only an owner can grant or remove the Owner role.")
        }
        AuthzError::LastOwner => ApiError::conflict("The instance must keep at least one owner."),
        AuthzError::ServiceAccountOwner => {
            ApiError::conflict("Service accounts can't hold the Owner role.")
        }
        AuthzError::Db(e) => ApiError::internal(&e),
    }
}

pub fn map_auth(error: AuthError) -> ApiError {
    match error {
        AuthError::EmailTaken => ApiError::conflict("A user with this email already exists."),
        AuthError::InvalidEmail => {
            ApiError::validation().with_field_error("/email", "Enter a valid email address.")
        }
        AuthError::InvalidDisplayName => ApiError::validation()
            .with_field_error("/displayName", "Enter a name of 1 to 200 characters."),
        AuthError::WeakPassword(e) => {
            ApiError::validation().with_field_error("/password", e.to_string())
        }
        AuthError::UserNotFound | AuthError::SessionNotFound => ApiError::not_found(),
        other => ApiError::internal(&other),
    }
}
