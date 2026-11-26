// The built-in "Remind people about days logged short" workflow
// (huxley/timesheet-reminders), written with the SDK.
import {
	expr,
	forEach,
	ref,
	step,
	text,
	trigger,
	workflow,
	type Ref,
} from "@usehuxley/sdk";

const person = ref.at("person");
/** Minutes as `{{ … // 60 }}h {{ … % 60 }}m`. */
const hours = (minutes: Ref) =>
	`${expr`${minutes} // 60`}h ${expr`${minutes} % 60`}m`;

export default workflow({
	key: "timesheet-under-logged",
	name: "Remind people about days logged short",
	description:
		"Every morning at 9:00, tells people who logged less time yesterday than their working hours expect (not on time off), so they can fill it in while they remember.",
	triggers: [trigger.schedule({ cron: "0 9 * * *" })],
	steps: [
		step("find", "core.timesheet.check", { with: { check: "under_logged" } }),
		forEach(
			"remind",
			ref.steps.find.output.people,
			[
				step("tell", "core.notify", {
					with: {
						to: person.recipient,
						title: `Yesterday: ${hours(person.logged_minutes)} logged of ${hours(person.expected_minutes)}`,
						message: "Log the rest of your time while you remember it.",
						url: text`/time?week=${person.period_start}`,
					},
				}),
			],
			{ as: "person" },
		),
	],
	outputs: { reminded: expr`${ref.steps.find.output.people} | length` },
});
