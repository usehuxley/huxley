//! Calendar sync (`core.calendar.sync`, ADR-061): people's busy time from
//! Microsoft 365 (the `msgraph` integration's free/busy schedule, through
//! the configured connection) and from iCalendar feeds, stored as busy
//! blocks for a window around now.

use std::collections::BTreeMap;

use huxley_db::{OrgScope, begin_scoped};
use huxley_workforce::calendar::{self, Block, BlockStatus, Settings, Target};
use serde_json::{Value as Json, json};
use uuid::Uuid;

use crate::Engine;
use crate::walk::error;

fn internal(e: impl std::fmt::Display) -> Json {
    error("internal", e.to_string())
}

/// Microsoft Graph asks for at most this many schedules a call.
const GRAPH_BATCH: usize = 20;

/// `2026-12-01T09:00:00.0000000` (UTC, as asked for) → unix seconds.
fn graph_time(value: &Json) -> Option<i64> {
    let text = value["dateTime"].as_str()?;
    let civil = jiff::civil::DateTime::strptime("%Y-%m-%dT%H:%M:%S", text.get(..19)?).ok()?;
    let zone = value["timeZone"].as_str().unwrap_or("UTC");
    let tz = jiff::tz::TimeZone::get(zone).unwrap_or(jiff::tz::TimeZone::UTC);
    Some(civil.to_zoned(tz).ok()?.timestamp().as_second())
}

fn iso(at: i64) -> String {
    jiff::Timestamp::from_second(at)
        .map(|t| t.strftime("%Y-%m-%dT%H:%M:%S").to_string())
        .unwrap_or_default()
}

/// Schedule items as blocks (private ones lose their subject).
fn graph_blocks(items: &Json) -> Vec<Block> {
    items
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let status = BlockStatus::from_graph(item["status"].as_str()?)?;
            let subject = if item["isPrivate"].as_bool() == Some(true) {
                None
            } else {
                item["subject"]
                    .as_str()
                    .map(|s| s.chars().take(500).collect())
            };
            Some(Block {
                starts_at: graph_time(&item["start"])?,
                ends_at: graph_time(&item["end"])?,
                status,
                subject,
            })
        })
        .collect()
}

/// What a sync read for one person: their blocks, or why it failed.
type Read = Result<Vec<Block>, String>;

impl Engine {
    /// `core.calendar.sync`: replaces busy blocks for everyone synced (or
    /// `user_id`), from a day back to [`calendar::WINDOW_DAYS`] ahead.
    /// Output: `{ people, blocks, failed: [{ user_id, error }] }`.
    pub(crate) async fn calendar_sync(&self, input: &Json) -> Result<Json, Json> {
        let only = match input["user_id"].as_str().filter(|s| !s.is_empty()) {
            Some(s) => Some(Uuid::parse_str(s).map_err(|_| error("input", "user_id isn't an id"))?),
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
        let settings = calendar::settings(&mut tx).await.map_err(internal)?;
        let mut targets = calendar::targets(&mut tx, settings.is_some())
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        if let Some(user) = only {
            targets.retain(|t| t.user_id == user);
        }
        let now = jiff::Timestamp::now().as_second();
        let window = (now - 86_400, now + calendar::WINDOW_DAYS * 86_400);

        let mut read: BTreeMap<Uuid, Read> = BTreeMap::new();
        if let Some(settings) = &settings {
            let provider: Vec<&Target> =
                targets.iter().filter(|t| t.source == "provider").collect();
            for batch in provider.chunks(GRAPH_BATCH) {
                read.extend(self.graph_schedules(settings, batch, window).await);
            }
        }
        for target in targets.iter().filter(|t| t.source == "ics") {
            read.insert(target.user_id, self.feed(target, window).await);
        }

        let keep_subjects = settings.as_ref().is_some_and(|s| s.keep_subjects);
        let (mut blocks, mut failed) = (0, Vec::new());
        for (user, result) in &read {
            let mut tx = begin_scoped(self.db(), &OrgScope::System)
                .await
                .map_err(internal)?;
            match result {
                Ok(found) => {
                    blocks += calendar::replace(&mut tx, *user, window, found, keep_subjects)
                        .await
                        .map_err(internal)?;
                    calendar::record(&mut tx, *user, None)
                        .await
                        .map_err(internal)?;
                }
                Err(why) => {
                    calendar::record(&mut tx, *user, Some(why))
                        .await
                        .map_err(internal)?;
                    failed.push(json!({ "user_id": user, "error": why }));
                }
            }
            tx.commit().await.map_err(internal)?;
        }
        Ok(json!({ "people": read.len(), "blocks": blocks, "failed": failed }))
    }

    /// Free/busy for up to 20 people in one call.
    async fn graph_schedules(
        &self,
        settings: &Settings,
        batch: &[&Target],
        (from, to): (i64, i64),
    ) -> Vec<(Uuid, Read)> {
        let addresses: Vec<&str> = batch.iter().map(|t| t.address.as_str()).collect();
        let request = json!({
            "action": "msgraph/getSchedule",
            "connection": settings.connection,
            "inputs": {
                "user": addresses[0],
                "Prefer": "outlook.timezone=\"UTC\"",
                "body": {
                    "schedules": addresses,
                    "startTime": { "dateTime": iso(from), "timeZone": "UTC" },
                    "endTime": { "dateTime": iso(to), "timeZone": "UTC" },
                    "availabilityViewInterval": 15
                }
            }
        });
        let response = match self.poll_action(settings.org_id, &request).await {
            Ok(Ok(body)) => body,
            Ok(Err(failure)) => {
                let why = format!(
                    "Microsoft Graph: {}",
                    failure["message"].as_str().unwrap_or("the call failed")
                );
                return batch
                    .iter()
                    .map(|t| (t.user_id, Err(why.clone())))
                    .collect();
            }
            Err(e) => {
                let why = e.to_string();
                return batch
                    .iter()
                    .map(|t| (t.user_id, Err(why.clone())))
                    .collect();
            }
        };
        let by_address: BTreeMap<String, &Json> = response["value"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| Some((s["scheduleId"].as_str()?.to_lowercase(), s)))
            .collect();
        batch
            .iter()
            .map(|t| {
                let result = match by_address.get(&t.address.to_lowercase()) {
                    None => Err("Microsoft Graph didn't return this calendar".to_owned()),
                    Some(s) if s["error"].is_object() => Err(format!(
                        "Microsoft Graph: {}",
                        s["error"]["message"]
                            .as_str()
                            .unwrap_or("no access to this calendar")
                    )),
                    Some(s) => Ok(graph_blocks(&s["scheduleItems"])),
                };
                (t.user_id, result)
            })
            .collect()
    }

    /// Busy time from someone's iCalendar feed.
    async fn feed(&self, target: &Target, (from, to): (i64, i64)) -> Read {
        let sealed = target.ics_url.as_deref().ok_or("No feed URL is set")?;
        let url = self
            .crypto
            .decrypt(calendar::ics_binding(target.user_id), sealed)
            .await
            .map_err(|_| "The feed URL couldn't be read".to_owned())?;
        let url = String::from_utf8_lossy(&url).into_owned();
        let url = url
            .strip_prefix("webcal://")
            .map_or(url.clone(), |rest| format!("https://{rest}"));
        let response = crate::http::request(&self.egress, &json!({ "method": "GET", "url": url }))
            .await
            .map_err(|failure| {
                format!(
                    "The feed: {}",
                    failure["message"]
                        .as_str()
                        .unwrap_or("it couldn't be fetched")
                )
            })?;
        let text = response["body"].as_str().ok_or("The feed isn't text")?;
        if !text.contains("BEGIN:VCALENDAR") {
            return Err("That address isn't an iCalendar feed".into());
        }
        Ok(huxley_workforce::ics::busy(
            text,
            from,
            to,
            &target.timezone,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_schedule_items_become_blocks() {
        let items = json!([
            { "status": "busy", "subject": "Board", "start": { "dateTime": "2026-12-01T15:00:00.0000000", "timeZone": "UTC" },
              "end": { "dateTime": "2026-12-01T16:00:00.0000000", "timeZone": "UTC" } },
            { "status": "free", "start": { "dateTime": "2026-12-01T17:00:00", "timeZone": "UTC" },
              "end": { "dateTime": "2026-12-01T18:00:00", "timeZone": "UTC" } },
            { "status": "oof", "isPrivate": true, "subject": "Dentist",
              "start": { "dateTime": "2026-12-02T09:00:00", "timeZone": "UTC" }, "end": { "dateTime": "2026-12-02T10:00:00", "timeZone": "UTC" } }
        ]);
        let blocks = graph_blocks(&items);
        assert_eq!(blocks.len(), 2, "free time isn't a block");
        assert_eq!(
            (blocks[0].status, blocks[0].subject.as_deref()),
            (BlockStatus::Busy, Some("Board"))
        );
        assert_eq!(blocks[0].ends_at - blocks[0].starts_at, 3600);
        assert_eq!(
            (blocks[1].status, blocks[1].subject.as_deref()),
            (BlockStatus::Oof, None)
        );
        assert_eq!(iso(blocks[0].starts_at), "2026-12-01T15:00:00");
    }
}
