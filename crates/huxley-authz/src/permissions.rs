//! The permission registry: every permission Huxley checks, in one place.
//!
//! Feature modules add their permissions here as they arrive. A role holding
//! `*` has every permission, including ones added later.

use serde::Serialize;
use utoipa::ToSchema;

/// Where a permission is evaluated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PermissionScope {
    /// Checked against a specific organization (and inherited grants above it).
    Org,
    /// Instance-wide: checked against the root organization.
    Instance,
}

#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Permission {
    pub key: &'static str,
    pub description: &'static str,
    pub scope: PermissionScope,
}

macro_rules! permissions {
    ($( $ident:ident = $key:literal, $scope:ident, $desc:literal; )*) => {
        $( pub const $ident: &str = $key; )*
        /// Every permission, in display order.
        pub const ALL: &[Permission] = &[
            $( Permission { key: $key, description: $desc, scope: PermissionScope::$scope }, )*
        ];
    };
}

permissions! {
    ORG_READ = "org:read", Org, "View organizations";
    ORG_CREATE = "org:create", Org, "Create sub-organizations";
    ORG_UPDATE = "org:update", Org, "Rename organizations";
    ORG_MOVE = "org:move", Org, "Move organizations within the tree";
    ORG_ARCHIVE = "org:archive", Org, "Archive organizations";
    ORG_SETTINGS_MANAGE = "org:settings:manage", Org, "Change organization settings, such as branding and numbering, and lock them for sub-organizations";
    USER_READ = "user:read", Instance, "View users";
    USER_CREATE = "user:create", Instance, "Create users";
    USER_UPDATE = "user:update", Instance, "Edit users and enable or disable them";
    ROLE_READ = "role:read", Instance, "View roles";
    ROLE_MANAGE = "role:manage", Instance, "Create, edit, and delete custom roles";
    ROLE_ASSIGN = "role_assignment:manage", Org, "Grant and remove roles at an organization";
    AUDIT_READ = "audit:read", Instance, "View the audit log";
    MODULES_MANAGE = "platform:modules:manage", Instance, "Turn feature modules on or off";
    CUSTOM_FIELD_MANAGE = "custom_field:manage", Instance, "Define custom fields";
    TAG_MANAGE = "tag:manage", Instance, "Create, rename, and delete tags";
    TEAM_MANAGE = "team:manage", Instance, "Create teams and manage their members";
    SERVICE_ACCOUNT_MANAGE = "service_account:manage", Instance, "Create service accounts and manage their API tokens";
    SECURITY_POLICY_MANAGE = "security:policy:manage", Instance, "Set sign-in security policy, such as requiring two-factor authentication";
    WORKFLOW_READ = "workflow:read", Org, "View workflows and their runs";
    WORKFLOW_WRITE = "workflow:write", Org, "Create and edit workflow drafts, and test-run them";
    WORKFLOW_PUBLISH = "workflow:publish", Org, "Publish and archive workflows";
    WORKFLOW_RUN = "workflow:run", Org, "Start and cancel runs of published workflows";
    RUNNERS_MANAGE = "platform:runners:manage", Instance, "Enrol, register and revoke script runners";
    DATASETS_SQL = "platform:datasets:sql", Instance, "Run read-only SQL over every organization's dataset rows";
    JOBS_MANAGE = "platform:jobs:manage", Instance, "See workflow jobs that failed repeatedly and retry them";
    INTEGRATION_READ = "integration:read", Org, "View integrations and their actions";
    INTEGRATION_MANAGE = "integration:manage", Org, "Import integrations from OpenAPI specs and update them";
    CONNECTION_READ = "connection:read", Org, "View connections (never their secrets)";
    CONNECTION_MANAGE = "connection:manage", Org, "Create connections and change their credentials";
    FORM_SUBMIT = "form:submit", Org, "Fill in forms, which starts their workflows";
    FORM_MANAGE = "form:manage", Org, "Create and change forms";
    SCRIPT_READ = "script:read", Org, "View scripts in the library";
    SCRIPT_MANAGE = "script:manage", Org, "Add scripts to the library and change them";
    PACK_INSTALL = "pack:install", Org, "Install, upgrade and export packs";
    DATASET_READ = "dataset:read", Org, "View datasets and their rows";
    DATASET_MANAGE = "dataset:manage", Org, "Create and change datasets, and write their rows";
    AI_MANAGE = "ai:manage", Org, "Set up the AI provider (with its key and budget) and see AI usage";
    AI_USE = "ai:use", Org, "Use the AI assistant (it acts with your own permissions)";
    VARIABLE_MANAGE = "variable:manage", Org, "Set organization variables and secrets for workflows, and lock them for sub-organizations";
    CRM_READ = "crm:read", Org, "View company details, contacts and sites";
    CRM_MANAGE = "crm:manage", Org, "Create and change contacts and sites, and edit company details";
    TICKET_READ = "ticket:read", Org, "View tickets, their notes and activity";
    TICKET_WRITE = "ticket:write", Org, "Create and work tickets: change them, add notes, change status, assign";
    SERVICEDESK_CONFIGURE = "servicedesk:configure", Instance, "Set up service queues, SLA policies and mailboxes";
    MAILBOX_INGEST = "mailbox:ingest", Instance, "Post email to service desk mailboxes (for a mail server's forwarding)";
    WORKFORCE_READ = "workforce:read", Instance, "See people's skills, certifications and availability";
    WORKFORCE_MANAGE = "workforce:manage", Instance, "Keep the skills and certifications catalogs, verify skills, and set what work requires";
    KB_READ = "kb:read", Org, "Read published knowledge base articles";
    KB_WRITE = "kb:write", Org, "Write knowledge base articles and submit them for review";
    KB_PUBLISH = "kb:publish", Org, "Approve, reject, publish and archive knowledge base articles, and keep their categories";
    VAULT_READ = "vault:read", Org, "See credential vault items (never their secrets) and who revealed them";
    VAULT_REVEAL = "vault:reveal", Org, "Reveal credential vault secrets (each reveal is logged and audited)";
    ASSET_READ = "asset:read", Org, "View assets, their relationships and impact";
    ASSET_WRITE = "asset:write", Org, "Add, change, relate and delete assets";
    CMDB_CONFIGURE = "cmdb:configure", Instance, "Keep asset types and their fields";
    VAULT_MANAGE = "vault:manage", Org, "Add, change and delete credential vault items and their secrets";
}

/// Wildcard granting every permission.
pub const WILDCARD: &str = "*";

/// The registry entry for a permission key.
pub fn get(key: &str) -> Option<&'static Permission> {
    ALL.iter().find(|p| p.key == key)
}

/// Whether a permission key is registered (or the wildcard).
pub fn is_known(key: &str) -> bool {
    key == WILDCARD || ALL.iter().any(|p| p.key == key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_unique_and_well_formed() {
        let mut keys: Vec<_> = ALL.iter().map(|p| p.key).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), ALL.len(), "duplicate permission key");
        assert!(ALL.iter().all(|p| p.key.contains(':')));
        assert!(is_known(ORG_READ) && is_known(WILDCARD) && !is_known("org:explode"));
    }
}
