// The built-in "Remind people to add notes to their time" workflow
// (huxley/timesheet-reminders), written with the SDK.
import { forEach, ref, step, text, trigger, workflow } from "@usehuxley/sdk";

const person = ref.at("person");

export default workflow({
	key: "timesheet-missing-notes",
	name: "Remind people to add notes to their time",
	description:
		"Every Friday at 15:00, tells people how many of this period's time entries have no notes, before they submit their timesheet.",
	triggers: [trigger.schedule({ cron: "0 15 * * 5" })],
	steps: [
		step("find", "core.timesheet.check", { with: { check: "missing_notes" } }),
		forEach(
			"remind",
			ref.steps.find.output.people,
			[
				step("tell", "core.notify", {
					with: {
						to: person.recipient,
						title: text`${person.missing_notes} time {{ 'entry has' if person.missing_notes == 1 else 'entries have' }} no notes this period`,
						message:
							"Notes tell your lead and the client what the time was for.",
						url: text`/time?week=${person.period_start}`,
					},
				}),
			],
			{ as: "person" },
		),
	],
	outputs: { reminded: "{{ steps.find.output.people | length }}" },
});
