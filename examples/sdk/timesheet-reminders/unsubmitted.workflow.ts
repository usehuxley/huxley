// The built-in "Remind people to submit their timesheets" workflow
// (huxley/timesheet-reminders), written with the SDK.
import {
	defineInputs,
	expr,
	forEach,
	input,
	ref,
	step,
	text,
	trigger,
	when,
	workflow,
} from "@usehuxley/sdk";

const person = ref.at("person");
const inputs = defineInputs({
	lead_after_days: input
		.integer({
			title: "Tell leads from this many days late",
			default: 3,
			minimum: 0,
		})
		.optional(),
});

export default workflow({
	key: "timesheet-unsubmitted",
	name: "Remind people to submit their timesheets",
	description:
		"Every weekday at 9:00, reminds people whose timesheet for the last period isn't submitted (or was rejected and not resubmitted). From three days late, their team leads are told too.",
	triggers: [trigger.schedule({ cron: "0 9 * * 1-5" })],
	inputs: inputs.schema,
	steps: [
		step("find", "core.timesheet.check", {
			with: { check: "unsubmitted", days_late: 1 },
		}),
		forEach(
			"remind",
			ref.steps.find.output.people,
			[
				step("tell", "core.notify", {
					with: {
						to: person.recipient,
						title: text`Your timesheet for ${person.period_start} to ${person.period_end} {{ 'was rejected: please fix and resubmit it' if person.status == 'rejected' else 'isn\\'t submitted' }}`,
						message: text`{{ person.logged_minutes // 60 }}h {{ person.logged_minutes % 60 }}m logged; the period ended ${person.days_late} day{{ '' if person.days_late == 1 else 's' }} ago.`,
						url: text`/time?week=${person.period_start}`,
					},
				}),
				when(
					"late",
					expr`${person.days_late} >= (${inputs.refs.lead_after_days} | default(3)) and ${person.lead_recipients}`,
					[
						step("tell_leads", "core.notify", {
							with: {
								to: person.lead_recipients,
								title: text`${person.person}'s timesheet for ${person.period_start} to ${person.period_end} is ${person.days_late} days late`,
								message: text`It's ${person.status}.`,
								url: "/timesheets",
							},
						}),
					],
				),
			],
			{ as: "person" },
		),
	],
	outputs: { reminded: expr`${ref.steps.find.output.people} | length` },
});
