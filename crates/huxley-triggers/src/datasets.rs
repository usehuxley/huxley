//! Starting dataset syncs: by hand, and on their schedules (with the other
//! schedules, on the scheduler leader).

use huxley_datasets::DatasetError;
use huxley_datasets::sync::{self, SyncConfig};
use huxley_db::{OrgScope, begin_scoped};
use huxley_engine::{Engine, StartRun};
use serde_json::json;
use sqlx::PgConnection;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{BATCH, TriggerError, TriggerResult};

fn dataset_error(e: DatasetError) -> TriggerError {
    match e {
        DatasetError::Db(e) => TriggerError::Db(e),
        other => TriggerError::Db(sqlx::Error::Protocol(other.to_string())),
    }
}

/// When `config`'s schedule next runs after `after`, in `org_id`'s zone.
pub async fn next_sync(
    conn: &mut PgConnection,
    org_id: Uuid,
    config: &SyncConfig,
    after: OffsetDateTime,
) -> TriggerResult<Option<OffsetDateTime>> {
    let Some(schedule) = &config.schedule else {
        return Ok(None);
    };
    let trigger = schedule.trigger();
    let zone = huxley_workflows::triggers::schedule_zone(conn, org_id, &trigger).await?;
    Ok(huxley_workflows::triggers::next_run(&trigger, &zone, after))
}

/// Starts a sync of dataset `id`: a run in each of its target
/// organizations. Returns the runs.
pub async fn start_sync(
    engine: &Engine,
    conn: &mut PgConnection,
    id: Uuid,
) -> TriggerResult<Vec<Uuid>> {
    let dataset = huxley_datasets::get(&mut *conn, id)
        .await
        .map_err(dataset_error)?;
    let Some(config) = dataset.sync(&mut *conn).await.map_err(dataset_error)? else {
        return Ok(Vec::new());
    };
    let document = sync::document(&dataset, &config);
    let mut runs = Vec::new();
    for org_id in sync::targets(&mut *conn, &dataset, config.orgs)
        .await
        .map_err(dataset_error)?
    {
        runs.push(
            engine
                .start_run(
                    &mut *conn,
                    StartRun {
                        org_id,
                        workflow_id: None,
                        version: None,
                        document: Some(document.clone()),
                        trigger_type: "dataset_sync".into(),
                        trigger: json!({ "dataset": dataset.id, "key": dataset.key }),
                        inputs: json!({}),
                        actor_id: None,
                    },
                )
                .await?,
        );
    }
    Ok(runs)
}

/// Starts every dataset sync that's due. Returns how many runs started.
pub async fn tick(engine: &Engine) -> TriggerResult<usize> {
    let mut tx = begin_scoped(engine.db(), &OrgScope::System).await?;
    let due = sync::due(&mut tx, BATCH).await.map_err(dataset_error)?;
    let now = OffsetDateTime::now_utc();
    let mut started = 0;
    for id in due {
        let dataset = huxley_datasets::get(&mut tx, id)
            .await
            .map_err(dataset_error)?;
        let next = match dataset.sync(&mut tx).await.map_err(dataset_error)? {
            Some(config) => next_sync(&mut tx, dataset.org_id, &config, now).await?,
            None => None,
        };
        started += start_sync(engine, &mut tx, id).await?.len();
        sync::started(&mut tx, id, next)
            .await
            .map_err(dataset_error)?;
    }
    tx.commit().await?;
    Ok(started)
}
