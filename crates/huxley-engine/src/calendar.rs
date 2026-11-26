//! Calendar sync (`core.calendar.sync`, ADR-061): people's busy time from
//! Microsoft 365 (the `msgraph` integration's free/busy schedule, through
//! the configured connection) and from iCalendar feeds, stored as busy
//! blocks for a window around now.

use std::collections::BTreeMap;

use huxley_db::outbox::{self, DomainEvent};
use huxley_db::{OrgScope, begin_scoped};
use huxley_workforce::calendar::{self, Block, BlockStatus, Settings, Target};
use serde_json::{Value as Json, json};
use time::OffsetDateTime;
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
        let (mut blocks, mut failed, mut conflicts) = (0, Vec::new(), 0);
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
                    conflicts += report_conflicts(&mut tx, *user).await.map_err(internal)?;
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
        Ok(
            json!({ "people": read.len(), "blocks": blocks, "failed": failed, "conflicts": conflicts }),
        )
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

/// A clash between someone's busy time and one of their appointments.
type Clash = (
    Uuid,
    Uuid,
    Uuid,
    OffsetDateTime,
    OffsetDateTime,
    OffsetDateTime,
    OffsetDateTime,
    String,
);

/// Raises `calendar.conflict` for each of someone's upcoming appointments
/// that their busy time newly overlaps (once per appointment and block;
/// Huxley's own written events aren't conflicts). Returns how many.
async fn report_conflicts(conn: &mut sqlx::PgConnection, user: Uuid) -> Result<usize, sqlx::Error> {
    let clashes: Vec<Clash> = sqlx::query_as(
        "SELECT a.id, a.org_id, a.ticket_id, a.starts_at, a.ends_at, b.starts_at, b.ends_at, b.status \
           FROM sd_appointments a JOIN wf_busy_blocks b ON b.user_id = a.user_id \
          WHERE a.user_id = $1 AND a.ends_at > now() AND b.status IN ('busy', 'oof') \
            AND b.starts_at < a.ends_at AND b.ends_at > a.starts_at \
            AND NOT EXISTS (SELECT 1 FROM wf_calendar_writes w WHERE w.user_id = b.user_id \
                             AND w.starts_at = b.starts_at AND w.ends_at = b.ends_at)",
    )
    .bind(user)
    .fetch_all(&mut *conn)
    .await?;
    let at = |t: OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let mut raised = 0;
    for (appointment, org, ticket, a_start, a_end, b_start, b_end, status) in clashes {
        let fresh: Option<Uuid> = sqlx::query_scalar(
            "INSERT INTO wf_calendar_conflicts (appointment_id, starts_at, ends_at) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING RETURNING appointment_id",
        )
        .bind(appointment)
        .bind(b_start)
        .bind(b_end)
        .fetch_optional(&mut *conn)
        .await?;
        if fresh.is_none() {
            continue;
        }
        outbox::emit(
            &mut *conn,
            DomainEvent {
                event_type: "calendar.conflict",
                org_id: Some(org),
                subject_type: "appointment",
                subject_id: appointment,
                payload: json!({
                    "ticketId": ticket, "userId": user,
                    "appointmentStartsAt": at(a_start), "appointmentEndsAt": at(a_end),
                    "busyStartsAt": at(b_start), "busyEndsAt": at(b_end), "busyStatus": status,
                }),
            },
        )
        .await
        .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        raised += 1;
    }
    Ok(raised)
}

/// A Graph call through the calendar connection: the body, or the status
/// (if the server answered) and why it failed.
type GraphResult = Result<Json, (Option<u64>, String)>;

impl Engine {
    async fn graph(&self, settings: &Settings, action: &str, inputs: Json) -> GraphResult {
        let request = json!({ "action": format!("msgraph/{action}"), "connection": settings.connection, "inputs": inputs });
        match self.poll_action(settings.org_id, &request).await {
            Ok(Ok(body)) => Ok(body),
            Ok(Err(failure)) => Err((
                failure["status"].as_u64(),
                format!(
                    "Microsoft Graph: {}",
                    failure["message"].as_str().unwrap_or("the call failed")
                ),
            )),
            Err(e) => Err((None, e.to_string())),
        }
    }

    /// `core.calendar.write`: puts an appointment in its person's calendar
    /// (Microsoft 365, when writing back is on), keeps it in step when it
    /// moves or changes hands, and removes it when it's cancelled. Output:
    /// `{ written, action, event_id }` or `{ written: false, reason }`.
    pub(crate) async fn calendar_write(&self, input: &Json) -> Result<Json, Json> {
        let id = input["appointment_id"]
            .as_str()
            .and_then(|s| Uuid::parse_str(s).ok())
            .ok_or_else(|| error("input", "appointment_id is required"))?;
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        let settings = calendar::settings(&mut tx).await.map_err(internal)?;
        let Some(settings) = settings.filter(|s| s.write_back) else {
            return Ok(
                json!({ "written": false, "reason": "Writing appointments to calendars is off" }),
            );
        };
        let current = match huxley_servicedesk::appointments::get(&mut tx, id).await {
            Ok(a) => Some(a),
            Err(huxley_servicedesk::DeskError::NotFound) => None,
            Err(e) => return Err(internal(e)),
        };
        let written = calendar::written(&mut tx, id).await.map_err(internal)?;
        let address = match &current {
            Some(a) => calendar::provider_address(&mut tx, a.user_id)
                .await
                .map_err(internal)?,
            None => None,
        };
        tx.commit().await.map_err(internal)?;

        // Gone, or now someone else's (or another calendar's): remove the old one.
        let same = written.as_ref().filter(|w| {
            current.as_ref().is_some_and(|a| a.user_id == w.user_id)
                && address.as_deref() == Some(w.address.as_str())
        });
        if let Some(w) = written.as_ref().filter(|_| same.is_none()) {
            self.remove_event(&settings, w).await?;
        }
        let Some(a) = current else {
            return Ok(
                json!({ "written": false, "action": if written.is_some() { "removed" } else { "none" } }),
            );
        };
        let Some(address) = address else {
            return Ok(
                json!({ "written": false, "reason": "Their calendar isn't synced from Microsoft 365" }),
            );
        };
        let event = self.event_for(&a);
        let (event_id, action) = match same {
            Some(w) => {
                let updated = self
                    .graph(
                        &settings,
                        "updateEvent",
                        json!({ "user": address, "event": w.event_id, "body": event.clone() }),
                    )
                    .await;
                match updated {
                    Ok(_) => (w.event_id.clone(), "updated"),
                    // Deleted from the calendar by hand: write it again.
                    Err((Some(404), _)) => (
                        self.create_event(&settings, &address, id, &event).await?,
                        "created",
                    ),
                    Err((_, why)) => return Err(error("calendar", why)),
                }
            }
            None => (
                self.create_event(&settings, &address, id, &event).await?,
                "created",
            ),
        };
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        calendar::record_written(
            &mut tx,
            &calendar::Written {
                appointment_id: id,
                user_id: a.user_id,
                address,
                event_id: event_id.clone(),
            },
            (a.starts_at, a.ends_at),
        )
        .await
        .map_err(internal)?;
        tx.commit().await.map_err(internal)?;
        Ok(json!({ "written": true, "action": action, "event_id": event_id }))
    }

    /// Removes an appointment's event from the calendar (gone already is
    /// fine) and forgets it.
    async fn remove_event(&self, settings: &Settings, w: &calendar::Written) -> Result<(), Json> {
        let removed = self
            .graph(
                settings,
                "deleteEvent",
                json!({ "user": w.address, "event": w.event_id }),
            )
            .await;
        if let Err((status, why)) = removed
            && status != Some(404)
        {
            return Err(error("calendar", why));
        }
        let mut tx = begin_scoped(self.db(), &OrgScope::System)
            .await
            .map_err(internal)?;
        calendar::forget_written(&mut tx, w.appointment_id)
            .await
            .map_err(internal)?;
        tx.commit().await.map_err(internal)
    }

    /// The calendar event for an appointment: the ticket as its subject, the
    /// client as its location, and a link back.
    fn event_for(&self, a: &huxley_servicedesk::appointments::Appointment) -> Json {
        let at =
            |t: OffsetDateTime| json!({ "dateTime": iso(t.unix_timestamp()), "timeZone": "UTC" });
        let link = self
            .public_url
            .as_deref()
            .map(|u| format!("{}/tickets/{}", u.trim_end_matches('/'), a.ticket_id))
            .unwrap_or_default();
        let event = json!({
            "subject": format!("{} {}", a.ticket_reference, a.ticket_title),
            "body": { "contentType": "text", "content": format!("{}\n{}\n{link}", a.org_name, a.notes).trim().to_owned() },
            "start": at(a.starts_at),
            "end": at(a.ends_at),
            "location": { "displayName": a.org_name },
            "showAs": "busy",
            "categories": ["Huxley"],
        });
        event
    }

    /// Creates the event (idempotently: the appointment's id is its
    /// transaction id). Returns its id.
    async fn create_event(
        &self,
        settings: &Settings,
        address: &str,
        appointment: Uuid,
        event: &Json,
    ) -> Result<String, Json> {
        let mut body = event.clone();
        body["transactionId"] = json!(appointment);
        let made = self
            .graph(
                settings,
                "createEvent",
                json!({ "user": address, "body": body }),
            )
            .await
            .map_err(|(_, why)| error("calendar", why))?;
        made["id"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| error("calendar", "Microsoft Graph didn't return the event's id"))
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
