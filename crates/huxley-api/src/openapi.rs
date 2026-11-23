//! OpenAPI document.
//!
//! Served at `/api/v1/openapi.json` and exported with `huxley openapi`, which
//! is how `packages/api-client` generates its TypeScript types.

use axum::Json;
use utoipa::OpenApi;

use crate::audit_routes::{self, AuditEventView, AuditPage};
use crate::auth_routes::{
    self, AuthMethodsView, ChangePasswordRequest, LoginMfaRequest, LoginRequest, LoginResponse,
    MeView, MfaChallengeRequest, MfaMethodView, PasskeyAnswer, SessionView, UserView,
};
use crate::health::{self, HealthStatus};
use crate::mfa_routes::{
    self, ConfirmTotpRequest, MfaStatusView, PasswordConfirmation, RecoveryCodesView,
    TotpEnrollmentView,
};
use crate::module_routes::{self, SetModuleRequest, SetModuleResponse};
use crate::org_routes::{
    self, ArchiveOrgRequest, CreateOrgRequest, MoveOrgRequest, OrgKindInput, UpdateOrgRequest,
};
use crate::passkey_routes::{
    self, CeremonyView, FinishRegistrationRequest, PasskeyView, RegisteredPasskeyView,
    RenamePasskeyRequest,
};
use crate::problem::{ApiError, FieldError};
use crate::security_routes::{self, SetRequirementRequest};
use crate::user_routes::{
    self, CreateAssignmentRequest, CreateUserRequest, CreateUserResponse, UpdateUserRequest,
    UserPage,
};

#[derive(Debug, OpenApi)]
#[openapi(
    info(
        title = "Huxley API",
        description = "Huxley platform API. See https://docs.usehuxley.com for conventions.",
        license(name = "MIT", identifier = "MIT"),
    ),
    paths(
        health::healthz,
        health::readyz,
        auth_routes::login,
        auth_routes::login_mfa,
        auth_routes::login_mfa_passkey_options,
        auth_routes::login_passkey_options,
        auth_routes::login_passkey,
        auth_routes::methods,
        passkey_routes::list,
        passkey_routes::begin_registration,
        passkey_routes::finish_registration,
        passkey_routes::rename,
        passkey_routes::remove,
        mfa_routes::status,
        mfa_routes::begin_totp,
        mfa_routes::confirm_totp,
        mfa_routes::disable_totp,
        mfa_routes::regenerate_recovery_codes,
        auth_routes::logout,
        auth_routes::me,
        auth_routes::change_password,
        auth_routes::list_sessions,
        auth_routes::revoke_session,
        org_routes::list,
        org_routes::get_one,
        org_routes::create,
        org_routes::update,
        org_routes::move_org,
        org_routes::archive,
        user_routes::list_users,
        user_routes::get_user,
        user_routes::create_user,
        user_routes::update_user,
        user_routes::reset_mfa,
        user_routes::list_assignments,
        user_routes::create_assignment,
        user_routes::delete_assignment,
        user_routes::list_roles,
        user_routes::list_permissions,
        audit_routes::list,
        module_routes::list,
        module_routes::set,
        security_routes::get_policy,
        security_routes::set_role,
        security_routes::set_org,
    ),
    components(schemas(HealthStatus, ApiError, FieldError, LoginRequest, ChangePasswordRequest, UserView, SessionView,
        MeView,
        LoginResponse,
        LoginMfaRequest,
        PasskeyAnswer,
        MfaChallengeRequest,
        AuthMethodsView,
        PasskeyView,
        CeremonyView,
        FinishRegistrationRequest,
        RegisteredPasskeyView,
        RenamePasskeyRequest,
        MfaMethodView,
        MfaStatusView,
        PasswordConfirmation,
        TotpEnrollmentView,
        ConfirmTotpRequest,
        RecoveryCodesView,
        huxley_orgs::Organization,
        huxley_orgs::OrgKind,
        huxley_orgs::OrgStatus,
        CreateOrgRequest,
        OrgKindInput,
        UpdateOrgRequest,
        MoveOrgRequest,
        ArchiveOrgRequest,
        UserPage,
        CreateUserRequest,
        CreateUserResponse,
        UpdateUserRequest,
        CreateAssignmentRequest,
        huxley_authz::Role,
        huxley_authz::Assignment,
        huxley_authz::Permission,
        huxley_authz::PermissionScope,
        AuditEventView,
        AuditPage,
        huxley_modules::Module,
        huxley_modules::ModuleState,
        SetModuleRequest,
        SetModuleResponse,
        SetRequirementRequest,
        huxley_authz::mfa_policy::MfaPolicy,
        huxley_authz::mfa_policy::PolicyOrg,
    )),
    tags(
        (name = "system", description = "Health and platform metadata"),
        (name = "auth", description = "Sign-in, sessions, and the current user"),
        (name = "orgs", description = "The organization tree"),
        (name = "users", description = "Users, roles, and role assignments"),
        (name = "audit", description = "The audit log"),
        (name = "modules", description = "Feature modules that can be turned on or off"),
        (name = "security", description = "Sign-in security policy"),
    ),
)]
pub struct ApiDoc;

/// Serves the OpenAPI document.
pub async fn openapi_json() -> Json<utoipa::openapi::OpenApi> {
    Json(ApiDoc::openapi())
}

/// The OpenAPI document as pretty-printed JSON.
pub fn to_pretty_json() -> String {
    ApiDoc::openapi()
        .to_pretty_json()
        .expect("OpenAPI document must serialize")
}
