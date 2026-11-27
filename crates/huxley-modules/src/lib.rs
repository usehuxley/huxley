//! Feature module registry and toggles (design doc §8.0, ADR-017).
//!
//! Everything outside the core platform is a feature module. Each module's
//! manifest lives in [`REGISTRY`]. Admins record which modules they want; a
//! module is *active* when it is enabled **and** released in this version
//! of Huxley. Module code checks [`ModuleState::is_active`] (via the API's
//! module gate) before serving anything.

use std::collections::BTreeSet;

use huxley_core::RequestContext;
use huxley_db::DbError;
use huxley_db::audit::{self, AuditRecord};
use huxley_db::outbox::{self, DomainEvent};
use serde::Serialize;
use serde_json::json;
use sqlx::PgConnection;
use utoipa::ToSchema;
use uuid::Uuid;

/// A feature module's manifest.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Module {
    pub key: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Modules that must be enabled for this one to work.
    pub requires: &'static [&'static str],
    /// Implementation-plan phase in which the module ships.
    pub phase: u8,
    /// Whether this version of Huxley includes the module.
    pub released: bool,
}

/// Every feature module, in display order. The core platform (orgs, users,
/// RBAC, audit, automation engine, integrations, datasets, AI/MCP) is always
/// on and isn't listed.
pub const REGISTRY: &[Module] = &[
    Module {
        key: "crm",
        name: "Clients and contacts",
        description: "Companies, contacts, sites, and the client 360 view.",
        requires: &[],
        phase: 5,
        released: true,
    },
    Module {
        key: "servicedesk",
        name: "Service desk",
        description: "Tickets, service queues, SLAs, mailbox ingestion, and dispatch.",
        requires: &["crm"],
        phase: 5,
        released: true,
    },
    Module {
        key: "workforce",
        name: "Workforce",
        description: "Skills, certifications, availability, and timesheets.",
        requires: &[],
        phase: 5,
        released: true,
    },
    Module {
        key: "cmdb",
        name: "Assets and CMDB",
        description: "Assets, configuration items, relationships, and dependency maps.",
        requires: &["crm"],
        phase: 6,
        released: true,
    },
    Module {
        key: "knowledge",
        name: "Knowledge base",
        description: "Articles with review and revisions, categories, and search.",
        requires: &[],
        phase: 6,
        released: true,
    },
    Module {
        key: "vault",
        name: "Credential vault",
        description: "Client passwords, keys and secret notes, encrypted, with every reveal audited.",
        requires: &[],
        phase: 6,
        released: true,
    },
    Module {
        key: "contracts",
        name: "Contracts",
        description: "Agreements (coverage, rates, recurring lines, and how time falls under them) and the product catalog.",
        requires: &["crm"],
        phase: 6,
        released: true,
    },
    Module {
        key: "portal",
        name: "Client portal",
        description: "A portal where client contacts sign in to see and raise their support requests.",
        requires: &["crm"],
        phase: 7,
        released: true,
    },
    Module {
        key: "cx",
        name: "Customer experience",
        description: "CSAT, NPS, and client health scores.",
        requires: &["servicedesk"],
        phase: 8,
        released: true,
    },
    Module {
        key: "change",
        name: "Change and problem management",
        description: "Changes, CAB approvals, change calendar, and problems.",
        requires: &["servicedesk"],
        phase: 9,
        released: true,
    },
    Module {
        key: "projects",
        name: "Projects",
        description: "Projects, templates, resource planning, and project billing.",
        requires: &["crm", "servicedesk"],
        phase: 9,
        released: true,
    },
    Module {
        key: "inventory",
        name: "Inventory and procurement",
        description: "Stock, warehouses, and purchase orders.",
        requires: &["contracts"],
        phase: 10,
        released: true,
    },
    Module {
        key: "sales",
        name: "Sales and quoting",
        description: "Pipeline, CPQ, and sales orders.",
        requires: &["crm", "contracts"],
        phase: 10,
        released: true,
    },
    Module {
        key: "billing",
        name: "Invoicing",
        description: "Invoice generation and accounting sync.",
        requires: &["contracts"],
        phase: 11,
        released: true,
    },
    Module {
        key: "expenses",
        name: "Expenses",
        description: "Expenses with receipts, mileage, reimbursement and billing.",
        requires: &["crm"],
        phase: 12,
        released: true,
    },
];

pub fn find(key: &str) -> Option<&'static Module> {
    REGISTRY.iter().find(|m| m.key == key)
}

/// Named starting points offered at bootstrap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// No feature modules: Huxley as an automation engine.
    AutomationOnly,
    ServiceDesk,
    FullPsa,
}

impl Preset {
    pub fn modules(self) -> Vec<&'static str> {
        match self {
            Self::AutomationOnly => vec![],
            Self::ServiceDesk => vec![
                "crm",
                "servicedesk",
                "change",
                "workforce",
                "cmdb",
                "knowledge",
                "portal",
            ],
            Self::FullPsa => REGISTRY.iter().map(|m| m.key).collect(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "automation" | "automation-only" => Some(Self::AutomationOnly),
            "service-desk" | "servicedesk" => Some(Self::ServiceDesk),
            "full" | "full-psa" => Some(Self::FullPsa),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ModuleError {
    #[error("unknown module `{0}`")]
    Unknown(String),
    #[error("requires modules that are turned off: {}", .0.join(", "))]
    MissingDependencies(Vec<&'static str>),
    #[error("other enabled modules depend on it: {}", .0.join(", "))]
    HasDependents(Vec<&'static str>),
}

/// Every module `key` needs, transitively, excluding itself.
pub fn dependencies(key: &str) -> Vec<&'static str> {
    let mut out = BTreeSet::new();
    let mut stack: Vec<&str> = find(key).map(|m| m.requires.to_vec()).unwrap_or_default();
    while let Some(dep) = stack.pop() {
        if let Some(m) = find(dep)
            && out.insert(m.key)
        {
            stack.extend(m.requires.iter().copied());
        }
    }
    out.into_iter().collect()
}

/// Enabled modules that (transitively) depend on `key`.
pub fn dependents(key: &str, enabled: &BTreeSet<&str>) -> Vec<&'static str> {
    REGISTRY
        .iter()
        .filter(|m| m.key != key && enabled.contains(m.key) && dependencies(m.key).contains(&key))
        .map(|m| m.key)
        .collect()
}

/// The modules to turn on to enable `key`: `key` itself plus any missing
/// dependencies (when `include_dependencies`), in dependency order.
pub fn enable_plan(
    key: &str,
    enabled: &BTreeSet<&str>,
    include_dependencies: bool,
) -> Result<Vec<&'static str>, ModuleError> {
    let module = find(key).ok_or_else(|| ModuleError::Unknown(key.to_owned()))?;
    let missing: Vec<&'static str> = dependencies(key)
        .into_iter()
        .filter(|d| !enabled.contains(d))
        .collect();
    if !missing.is_empty() && !include_dependencies {
        return Err(ModuleError::MissingDependencies(missing));
    }
    // Dependencies first, ordered by how deep they sit in the graph.
    let mut plan = missing;
    plan.sort_by_key(|d| dependencies(d).len());
    plan.push(module.key);
    Ok(plan)
}

/// Checks that `key` can be turned off.
pub fn disable_check(key: &str, enabled: &BTreeSet<&str>) -> Result<(), ModuleError> {
    find(key).ok_or_else(|| ModuleError::Unknown(key.to_owned()))?;
    let blocking = dependents(key, enabled);
    if blocking.is_empty() {
        Ok(())
    } else {
        Err(ModuleError::HasDependents(blocking))
    }
}

/// A module's state for the instance.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ModuleState {
    #[serde(flatten)]
    pub module: Module,
    /// The admin wants it on.
    pub enabled: bool,
    /// Enabled and released: the module is running.
    pub active: bool,
}

impl ModuleState {
    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// Instance-wide (root org) desired state. Run in system scope.
pub async fn enabled_set(conn: &mut PgConnection) -> Result<BTreeSet<&'static str>, DbError> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT s.module_key FROM module_settings s \
           JOIN organizations o ON o.id = s.org_id AND o.parent_id IS NULL \
          WHERE s.enabled",
    )
    .fetch_all(conn)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(k,)| find(&k).map(|m| m.key))
        .collect())
}

/// Every module with its instance-wide state.
pub async fn states(conn: &mut PgConnection) -> Result<Vec<ModuleState>, DbError> {
    let enabled = enabled_set(conn).await?;
    Ok(REGISTRY
        .iter()
        .map(|m| {
            let on = enabled.contains(m.key);
            ModuleState {
                module: *m,
                enabled: on,
                active: on && m.released,
            }
        })
        .collect())
}

/// Whether a module is active instance-wide. Run in system scope.
pub async fn is_active(conn: &mut PgConnection, key: &str) -> Result<bool, DbError> {
    let Some(module) = find(key) else {
        return Ok(false);
    };
    Ok(module.released && enabled_set(conn).await?.contains(key))
}

#[derive(Debug, thiserror::Error)]
pub enum SetError {
    #[error(transparent)]
    Module(#[from] ModuleError),
    #[error("organization not found")]
    OrgNotFound,
    #[error("modules can only be narrowed at provider organizations below the root")]
    NotAProviderOrg,
    #[error("the module is turned off for the whole instance")]
    OffInstanceWide,
    #[error("{0} turned this module off for its organizations")]
    NarrowedAbove(String),
    #[error(transparent)]
    Db(#[from] DbError),
}

impl From<sqlx::Error> for SetError {
    fn from(e: sqlx::Error) -> Self {
        Self::Db(DbError::Sqlx(e))
    }
}

/// Turns a module on or off instance-wide. Returns the modules whose state
/// changed. Run in system scope after checking `platform:modules:manage`.
pub async fn set_enabled(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    key: &str,
    enabled: bool,
    include_dependencies: bool,
) -> Result<Vec<&'static str>, SetError> {
    // Serialize toggles so dependency checks see a consistent set.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('huxley_module_settings'))")
        .execute(&mut *conn)
        .await?;
    let current = enabled_set(conn).await?;
    let changes: Vec<&'static str> = if enabled {
        enable_plan(key, &current, include_dependencies)?
            .into_iter()
            .filter(|k| !current.contains(k))
            .collect()
    } else {
        disable_check(key, &current)?;
        if current.contains(key) {
            vec![find(key).expect("checked above").key]
        } else {
            vec![]
        }
    };

    let (root,): (Uuid,) = sqlx::query_as("SELECT id FROM organizations WHERE parent_id IS NULL")
        .fetch_one(&mut *conn)
        .await?;
    for module in &changes {
        sqlx::query(
            "INSERT INTO module_settings (org_id, module_key, enabled, updated_by) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (org_id, module_key) DO UPDATE \
               SET enabled = excluded.enabled, updated_at = now(), updated_by = excluded.updated_by",
        )
        .bind(root)
        .bind(module)
        .bind(enabled)
        .bind(ctx.actor.id())
        .execute(&mut *conn)
        .await?;
        let action = if enabled {
            "module.enabled"
        } else {
            "module.disabled"
        };
        audit::record(
            conn,
            ctx,
            AuditRecord::new(action)
                .org(root)
                .details(json!({ "module": module, "requestedFor": key })),
        )
        .await?;
        outbox::emit(
            conn,
            DomainEvent {
                event_type: action,
                org_id: Some(root),
                subject_type: "organization",
                subject_id: root,
                payload: json!({ "module": module }),
            },
        )
        .await?;
    }
    Ok(changes)
}

/// A module's state at one organization.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrgModuleState {
    #[serde(flatten)]
    pub module: Module,
    /// On for the instance.
    pub enabled: bool,
    /// On for this organization (instance-wide and not narrowed).
    pub enabled_here: bool,
    /// Running here: enabled here and released.
    pub active: bool,
    /// The provider org that turned it off here (this org or an ancestor).
    pub narrowed_by: Option<NarrowedBy>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NarrowedBy {
    pub org_id: Uuid,
    pub org_name: String,
}

/// Per-org overrides on `org_id`'s chain (itself and ancestors below the
/// root), shallowest first: (module, org id, org name).
async fn narrowings(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> Result<Vec<(String, Uuid, String)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.module_key, o.id, o.name FROM module_settings s \
           JOIN organizations o ON o.id = s.org_id AND o.parent_id IS NOT NULL \
          WHERE NOT s.enabled \
            AND o.path @> (SELECT path FROM organizations WHERE id = $1) \
          ORDER BY nlevel(o.path)",
    )
    .bind(org_id)
    .fetch_all(conn)
    .await?)
}

/// Modules on at `org_id`: the instance set minus modules a provider org on
/// its chain turned off. Run in system scope.
pub async fn enabled_set_at(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> Result<BTreeSet<&'static str>, DbError> {
    let mut set = enabled_set(conn).await?;
    for (key, _, _) in narrowings(conn, org_id).await? {
        set.remove(key.as_str());
    }
    Ok(set)
}

/// Whether a module is active at `org_id`. Run in system scope.
pub async fn is_active_at(
    conn: &mut PgConnection,
    key: &str,
    org_id: Uuid,
) -> Result<bool, DbError> {
    let Some(module) = find(key) else {
        return Ok(false);
    };
    Ok(module.released && enabled_set_at(conn, org_id).await?.contains(key))
}

/// Every module with its state at `org_id`.
pub async fn states_at(
    conn: &mut PgConnection,
    org_id: Uuid,
) -> Result<Vec<OrgModuleState>, DbError> {
    let instance = enabled_set(conn).await?;
    let narrowed = narrowings(conn, org_id).await?;
    Ok(REGISTRY
        .iter()
        .map(|m| {
            let narrowed_by = narrowed
                .iter()
                .find(|(k, _, _)| k == m.key)
                .map(|(_, id, name)| NarrowedBy {
                    org_id: *id,
                    org_name: name.clone(),
                });
            let enabled = instance.contains(m.key);
            let enabled_here = enabled && narrowed_by.is_none();
            OrgModuleState {
                module: *m,
                enabled,
                enabled_here,
                active: enabled_here && m.released,
                narrowed_by,
            }
        })
        .collect())
}

/// Turns a module off (or back on) for a provider org below the root and its
/// sub-organizations (design doc §8.0: narrowed per provider org). Only
/// narrows: a module off for the instance can't be turned on here. Run in
/// system scope after checking `platform:modules:manage`.
pub async fn set_for_org(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    org_id: Uuid,
    key: &str,
    enabled: bool,
) -> Result<bool, SetError> {
    let module = find(key).ok_or_else(|| ModuleError::Unknown(key.to_owned()))?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('huxley_module_settings'))")
        .execute(&mut *conn)
        .await?;
    let org: Option<(Option<Uuid>, String)> = sqlx::query_as(
        "SELECT parent_id, kind::text FROM organizations WHERE id = $1 AND status = 'active'",
    )
    .bind(org_id)
    .fetch_optional(&mut *conn)
    .await?;
    match org {
        None => return Err(SetError::OrgNotFound),
        Some((None, _)) => return Err(SetError::NotAProviderOrg),
        Some((Some(_), kind)) if kind != "provider" => return Err(SetError::NotAProviderOrg),
        Some(_) => {}
    }
    let here = enabled_set_at(conn, org_id).await?;
    let chain = narrowings(conn, org_id).await?;
    let own = chain
        .iter()
        .any(|(k, id, _)| k == module.key && *id == org_id);
    if enabled {
        if let Some((_, _, name)) = chain
            .iter()
            .find(|(k, id, _)| k == module.key && *id != org_id)
        {
            return Err(SetError::NarrowedAbove(name.clone()));
        }
        if !own {
            return Ok(false);
        }
        if !enabled_set(conn).await?.contains(module.key) {
            return Err(SetError::OffInstanceWide);
        }
        let missing: Vec<&'static str> = dependencies(module.key)
            .into_iter()
            .filter(|d| !here.contains(d))
            .collect();
        if !missing.is_empty() {
            return Err(ModuleError::MissingDependencies(missing).into());
        }
        sqlx::query("DELETE FROM module_settings WHERE org_id = $1 AND module_key = $2")
            .bind(org_id)
            .bind(module.key)
            .execute(&mut *conn)
            .await?;
    } else {
        if !here.contains(module.key) {
            return Ok(false);
        }
        disable_check(module.key, &here)?;
        sqlx::query(
            "INSERT INTO module_settings (org_id, module_key, enabled, updated_by) VALUES ($1, $2, false, $3) \
             ON CONFLICT (org_id, module_key) DO UPDATE SET enabled = false, updated_at = now(), updated_by = excluded.updated_by",
        )
        .bind(org_id)
        .bind(module.key)
        .bind(ctx.actor.id())
        .execute(&mut *conn)
        .await?;
    }
    let action = if enabled {
        "module.restored_for_org"
    } else {
        "module.narrowed_for_org"
    };
    audit::record(
        conn,
        ctx,
        AuditRecord::new(action)
            .org(org_id)
            .details(json!({ "module": module.key })),
    )
    .await?;
    outbox::emit(
        conn,
        DomainEvent {
            event_type: action,
            org_id: Some(org_id),
            subject_type: "organization",
            subject_id: org_id,
            payload: json!({ "module": module.key }),
        },
    )
    .await?;
    Ok(true)
}

/// Applies a bootstrap preset. Run in system scope on a new instance.
pub async fn apply_preset(
    conn: &mut PgConnection,
    ctx: &RequestContext,
    preset: Preset,
) -> Result<(), SetError> {
    for key in preset.modules() {
        set_enabled(conn, ctx, key, true, true).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(keys: &[&'static str]) -> BTreeSet<&'static str> {
        keys.iter().copied().collect()
    }

    #[test]
    fn registry_is_consistent() {
        let keys: BTreeSet<_> = REGISTRY.iter().map(|m| m.key).collect();
        assert_eq!(keys.len(), REGISTRY.len(), "duplicate module key");
        for m in REGISTRY {
            for dep in m.requires {
                assert!(keys.contains(dep), "{} requires unknown {dep}", m.key);
            }
            assert!(
                !dependencies(m.key).contains(&m.key),
                "{} depends on itself",
                m.key
            );
        }
    }

    #[test]
    fn dependencies_are_transitive() {
        assert_eq!(dependencies("cx"), vec!["crm", "servicedesk"]);
        assert_eq!(dependencies("crm"), Vec::<&str>::new());
        assert_eq!(dependencies("billing"), vec!["contracts", "crm"]);
    }

    #[test]
    fn enabling_requires_or_includes_dependencies() {
        assert_eq!(
            enable_plan("cx", &set(&[]), false),
            Err(ModuleError::MissingDependencies(vec!["crm", "servicedesk"]))
        );
        // Dependencies come first, deepest last-needed order.
        assert_eq!(
            enable_plan("cx", &set(&[]), true).unwrap(),
            vec!["crm", "servicedesk", "cx"]
        );
        assert_eq!(
            enable_plan("servicedesk", &set(&["crm"]), false).unwrap(),
            vec!["servicedesk"]
        );
        assert!(matches!(
            enable_plan("nope", &set(&[]), true),
            Err(ModuleError::Unknown(_))
        ));
    }

    #[test]
    fn disabling_is_blocked_by_enabled_dependents() {
        let enabled = set(&["crm", "servicedesk", "cx"]);
        assert_eq!(
            disable_check("crm", &enabled),
            Err(ModuleError::HasDependents(vec!["servicedesk", "cx"]))
        );
        assert_eq!(disable_check("cx", &enabled), Ok(()));
    }

    #[test]
    fn presets() {
        assert_eq!(Preset::AutomationOnly.modules(), Vec::<&str>::new());
        assert_eq!(Preset::FullPsa.modules().len(), REGISTRY.len());
        assert_eq!(Preset::parse("service-desk"), Some(Preset::ServiceDesk));
        assert_eq!(Preset::parse("everything"), None);
        // Service desk preset is self-consistent.
        let sd: BTreeSet<_> = Preset::ServiceDesk.modules().into_iter().collect();
        for key in &sd {
            for dep in dependencies(key) {
                assert!(sd.contains(dep), "{key} needs {dep}");
            }
        }
    }
}
