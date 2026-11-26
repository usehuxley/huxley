//! The `workforce` module's timesheet step (`core.timesheet.check`).

use huxley_db::{OrgScope, begin_scoped};
use huxley_workforce::timesheets;
use serde_json::{Value as Json, json};

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

impl Engine {
    /// `core.timesheet.check`: whom to remind about timesheets (unsubmitted
    /// ones, days logged under expected hours, entries missing notes).
    pub(crate) async fn check_timesheets(&self, input: &Json) -> Result<Json, Json> {
        let check = input["check"]
            .as_str()
            .ok_or_else(|| {
                error(
                    "input",
                    "check is unsubmitted, under_logged or missing_notes",
                )
            })?
            .to_owned();
        let days_late = input["days_late"].as_i64().unwrap_or(1).clamp(0, 60);
        let date = match input["date"].as_str().filter(|d| !d.is_empty()) {
            Some(d) => {
                Some(timesheets::parse_day("date", d).map_err(|e| error("input", e.to_string()))?)
            }
            None => None,
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        if !huxley_modules::is_active(&mut tx, huxley_workforce::MODULE)
            .await
            .map_err(internal)?
        {
            return Err(error(
                "module_disabled",
                "The workforce module is turned off",
            ));
        }
        let today = time::OffsetDateTime::now_utc().date();
        let people = timesheets::reminders(&mut tx, &check, days_late, date, today)
            .await
            .map_err(|e| match e {
                huxley_workforce::WorkforceError::Invalid { .. } => error("input", e.to_string()),
                other => internal(other),
            })?;
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "check": check, "people": people }))
    }
}
