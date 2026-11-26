// The SDK rewrites of the built-in timesheet reminders compile to exactly
// the pack's YAML workflows.
import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import { parse } from 'yaml';
import missingNotes from '../../../examples/sdk/timesheet-reminders/missing-notes.workflow.ts';
import underLogged from '../../../examples/sdk/timesheet-reminders/under-logged.workflow.ts';
import unsubmitted from '../../../examples/sdk/timesheet-reminders/unsubmitted.workflow.ts';

const pack = new URL(
	'../../../crates/huxley-packs/builtin/timesheet-reminders/workflows/',
	import.meta.url
);

describe('the timesheet reminders, written with the SDK', () => {
	for (const [file, built] of [
		['timesheet-missing-notes.yaml', missingNotes],
		['timesheet-under-logged.yaml', underLogged],
		['timesheet-unsubmitted.yaml', unsubmitted]
	] as const) {
		it(`match ${file}`, async () => {
			const yaml = parse(await readFile(new URL(file, pack), 'utf8')) as unknown;
			expect(JSON.parse(JSON.stringify(built))).toEqual(yaml);
		});
	}
});
