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
    USER_READ = "user:read", Instance, "View users";
    USER_CREATE = "user:create", Instance, "Create users";
    USER_UPDATE = "user:update", Instance, "Edit users and enable or disable them";
    ROLE_READ = "role:read", Instance, "View roles";
    ROLE_ASSIGN = "role_assignment:manage", Org, "Grant and remove roles at an organization";
    AUDIT_READ = "audit:read", Instance, "View the audit log";
    MODULES_MANAGE = "platform:modules:manage", Instance, "Turn feature modules on or off";
}

/// Wildcard granting every permission.
pub const WILDCARD: &str = "*";

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
