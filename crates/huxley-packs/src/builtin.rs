//! Built-in packs: shipped inside the binary, so trusted by construction.
//! A pack whose manifest and files are exactly a built-in one's is
//! recognised as built in wherever it comes from.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::{Manifest, Pack};

struct Source {
    manifest: &'static str,
    files: &'static [(&'static str, &'static str)],
}

const SOURCES: &[Source] = &[
    Source {
        manifest: include_str!("../builtin/servicedesk-email/pack.yaml"),
        files: &[
            (
                "workflows/email-to-ticket.yaml",
                include_str!("../builtin/servicedesk-email/workflows/email-to-ticket.yaml"),
            ),
            (
                "workflows/poll-mailboxes.yaml",
                include_str!("../builtin/servicedesk-email/workflows/poll-mailboxes.yaml"),
            ),
            (
                "workflows/email-replies.yaml",
                include_str!("../builtin/servicedesk-email/workflows/email-replies.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/servicedesk-dispatch/pack.yaml"),
        files: &[(
            "workflows/dispatch-tickets.yaml",
            include_str!("../builtin/servicedesk-dispatch/workflows/dispatch-tickets.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/servicedesk-notifications/pack.yaml"),
        files: &[
            (
                "workflows/notify-assignment.yaml",
                include_str!(
                    "../builtin/servicedesk-notifications/workflows/notify-assignment.yaml"
                ),
            ),
            (
                "workflows/notify-reply.yaml",
                include_str!("../builtin/servicedesk-notifications/workflows/notify-reply.yaml"),
            ),
            (
                "workflows/notify-sla.yaml",
                include_str!("../builtin/servicedesk-notifications/workflows/notify-sla.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/csat/pack.yaml"),
        files: &[
            (
                "workflows/csat-survey.yaml",
                include_str!("../builtin/csat/workflows/csat-survey.yaml"),
            ),
            (
                "workflows/csat-low-rating.yaml",
                include_str!("../builtin/csat/workflows/csat-low-rating.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/nps/pack.yaml"),
        files: &[
            (
                "workflows/nps-quarterly.yaml",
                include_str!("../builtin/nps/workflows/nps-quarterly.yaml"),
            ),
            (
                "workflows/nps-detractor.yaml",
                include_str!("../builtin/nps/workflows/nps-detractor.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/dashboard-publishing/pack.yaml"),
        files: &[
            (
                "workflows/dashboard-submitted.yaml",
                include_str!("../builtin/dashboard-publishing/workflows/dashboard-submitted.yaml"),
            ),
            (
                "workflows/dashboard-decided.yaml",
                include_str!("../builtin/dashboard-publishing/workflows/dashboard-decided.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/change-approvals/pack.yaml"),
        files: &[(
            "workflows/approve-change.yaml",
            include_str!("../builtin/change-approvals/workflows/approve-change.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/calendar-sync/pack.yaml"),
        files: &[(
            "workflows/sync-calendars.yaml",
            include_str!("../builtin/calendar-sync/workflows/sync-calendars.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/project-status-reports/pack.yaml"),
        files: &[(
            "workflows/weekly-status-reports.yaml",
            include_str!("../builtin/project-status-reports/workflows/weekly-status-reports.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/project-scope-approvals/pack.yaml"),
        files: &[(
            "workflows/approve-scope-change.yaml",
            include_str!("../builtin/project-scope-approvals/workflows/approve-scope-change.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/ai-auto-documentation/pack.yaml"),
        files: &[(
            "workflows/document-resolution.yaml",
            include_str!("../builtin/ai-auto-documentation/workflows/document-resolution.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/ai-ticket-insights/pack.yaml"),
        files: &[(
            "workflows/summarize-ticket.yaml",
            include_str!("../builtin/ai-ticket-insights/workflows/summarize-ticket.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/skill-gaps/pack.yaml"),
        files: &[(
            "workflows/skill-gaps-weekly.yaml",
            include_str!("../builtin/skill-gaps/workflows/skill-gaps-weekly.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/ai-triage/pack.yaml"),
        files: &[(
            "workflows/triage-ticket.yaml",
            include_str!("../builtin/ai-triage/workflows/triage-ticket.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/ai-sentiment/pack.yaml"),
        files: &[
            (
                "workflows/sentiment-judge.yaml",
                include_str!("../builtin/ai-sentiment/workflows/sentiment-judge.yaml"),
            ),
            (
                "workflows/sentiment-declining.yaml",
                include_str!("../builtin/ai-sentiment/workflows/sentiment-declining.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/client-health/pack.yaml"),
        files: &[
            (
                "workflows/health-daily.yaml",
                include_str!("../builtin/client-health/workflows/health-daily.yaml"),
            ),
            (
                "workflows/health-declining.yaml",
                include_str!("../builtin/client-health/workflows/health-declining.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/knowledge-search/pack.yaml"),
        files: &[(
            "workflows/embed-articles.yaml",
            include_str!("../builtin/knowledge-search/workflows/embed-articles.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/servicedesk-search/pack.yaml"),
        files: &[(
            "workflows/embed-tickets.yaml",
            include_str!("../builtin/servicedesk-search/workflows/embed-tickets.yaml"),
        )],
    },
    Source {
        manifest: include_str!("../builtin/servicedesk-chat/pack.yaml"),
        files: &[
            (
                "workflows/slack-ticket-alerts.yaml",
                include_str!("../builtin/servicedesk-chat/workflows/slack-ticket-alerts.yaml"),
            ),
            (
                "workflows/teams-ticket-alerts.yaml",
                include_str!("../builtin/servicedesk-chat/workflows/teams-ticket-alerts.yaml"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/timesheet-reminders/pack.yaml"),
        files: &[
            (
                "workflows/timesheet-unsubmitted.yaml",
                include_str!("../builtin/timesheet-reminders/workflows/timesheet-unsubmitted.yaml"),
            ),
            (
                "workflows/timesheet-under-logged.yaml",
                include_str!(
                    "../builtin/timesheet-reminders/workflows/timesheet-under-logged.yaml"
                ),
            ),
            (
                "workflows/timesheet-missing-notes.yaml",
                include_str!(
                    "../builtin/timesheet-reminders/workflows/timesheet-missing-notes.yaml"
                ),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/immybot/pack.yaml"),
        files: &[
            (
                "integrations/immybot.yaml",
                include_str!("../builtin/immybot/integrations/immybot.yaml"),
            ),
            (
                "workflows/immybot-run-maintenance.yaml",
                include_str!("../builtin/immybot/workflows/immybot-run-maintenance.yaml"),
            ),
            (
                "workflows/immybot-sync-computers.yaml",
                include_str!("../builtin/immybot/workflows/immybot-sync-computers.yaml"),
            ),
            (
                "datasets/immybot-computers.json",
                include_str!("../builtin/immybot/datasets/immybot-computers.json"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/cwautomate/pack.yaml"),
        files: &[
            (
                "integrations/cwautomate.yaml",
                include_str!("../builtin/cwautomate/integrations/cwautomate.yaml"),
            ),
            (
                "workflows/automate-run-command.yaml",
                include_str!("../builtin/cwautomate/workflows/automate-run-command.yaml"),
            ),
            (
                "workflows/automate-sync-computers.yaml",
                include_str!("../builtin/cwautomate/workflows/automate-sync-computers.yaml"),
            ),
            (
                "datasets/automate-computers.json",
                include_str!("../builtin/cwautomate/datasets/automate-computers.json"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/msgraph/pack.yaml"),
        files: &[
            (
                "integrations/msgraph.yaml",
                include_str!("../builtin/msgraph/integrations/msgraph.yaml"),
            ),
            (
                "integrations/msgraph.overlay.json",
                include_str!("../builtin/msgraph/integrations/msgraph.overlay.json"),
            ),
        ],
    },
    Source {
        manifest: include_str!("../builtin/slack/pack.yaml"),
        files: &[
            (
                "integrations/slack.yaml",
                include_str!("../builtin/slack/integrations/slack.yaml"),
            ),
            (
                "integrations/slack.overlay.json",
                include_str!("../builtin/slack/integrations/slack.overlay.json"),
            ),
        ],
    },
];

/// Each built-in pack, and its bytes.
fn all() -> &'static [(Pack, Vec<u8>)] {
    static PACKS: OnceLock<Vec<(Pack, Vec<u8>)>> = OnceLock::new();
    PACKS.get_or_init(|| {
        SOURCES
            .iter()
            .map(|source| {
                let manifest: Manifest =
                    serde_saphyr::from_str(source.manifest).expect("built-in manifests parse");
                let files: BTreeMap<String, Vec<u8>> = source
                    .files
                    .iter()
                    .map(|(path, text)| ((*path).to_owned(), text.as_bytes().to_vec()))
                    .collect();
                let bytes = crate::build(manifest, &files, None).expect("built-in packs build");
                let mut pack = Pack::read_unrecognised(&bytes).expect("built-in packs read");
                pack.builtin = true;
                (pack, bytes)
            })
            .collect()
    })
}

/// Every built-in pack.
pub fn list() -> Vec<Pack> {
    all().iter().map(|(pack, _)| pack.clone()).collect()
}

/// Built-in pack `key` (e.g. `huxley/msgraph`), as a `.hxpack`.
pub fn bytes(key: &str) -> Option<&'static [u8]> {
    all()
        .iter()
        .find(|(pack, _)| pack.manifest.key == key)
        .map(|(_, bytes)| bytes.as_slice())
}

/// The built-in pack that adds integration `key`.
pub fn for_integration(key: &str) -> Option<Pack> {
    list()
        .into_iter()
        .find(|p| p.manifest.contents.integrations.iter().any(|k| k == key))
}

/// Whether `pack` is exactly a built-in one.
pub(crate) fn is_builtin(pack: &Pack) -> bool {
    all()
        .iter()
        .any(|(b, _)| b.manifest == pack.manifest && b.files == pack.files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Trust;

    #[test]
    fn built_ins_are_recognised_wherever_they_come_from() {
        assert_eq!(list().len(), SOURCES.len());
        let bytes = bytes("huxley/msgraph").unwrap();
        let pack = Pack::read(bytes).unwrap();
        assert_eq!(
            pack.trust(&[]),
            Trust::BuiltIn {
                publisher: "Huxley".into()
            }
        );
        assert_eq!(
            for_integration("slack").unwrap().manifest.key,
            "huxley/slack"
        );
        // A changed copy is just an unsigned pack.
        let mut files = pack.files().clone();
        files.insert("integrations/msgraph.overlay.json".into(), b"{}".to_vec());
        let changed = crate::build(pack.manifest.clone(), &files, None).unwrap();
        assert_eq!(Pack::read(&changed).unwrap().trust(&[]), Trust::Unsigned);
    }
}
