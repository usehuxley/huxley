import { error, fail } from '@sveltejs/kit';
import { hoursFromForm } from '#lib/crm/hours.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { parseDuration } from '#lib/tickets/duration.ts';
import { PRIORITIES } from '#lib/tickets/labels.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/servicedesk/sla-policies');
	if (response.status === 404) error(404, 'The service desk is turned off.');
	if (!data) error(503, 'Could not load SLA policies.');
	return { policies: data };
};

type Targets = Record<string, { response: number | null; resolution: number | null }>;

/** Targets from the form's `response_p1`… fields, or the first unreadable field. */
function targetsFromForm(form: FormData): { targets: Targets } | { bad: string } {
	const targets: Targets = {};
	for (const { value } of PRIORITIES) {
		const response = parseDuration(formString(form, `response_${value}`));
		const resolution = parseDuration(formString(form, `resolution_${value}`));
		if (Number.isNaN(response)) return { bad: `response_${value}` };
		if (Number.isNaN(resolution)) return { bad: `resolution_${value}` };
		if (response !== null || resolution !== null) targets[value] = { response, resolution };
	}
	return { targets };
}

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const parsed = targetsFromForm(form);
		if ('bad' in parsed) {
			return fail(422, {
				errors: { [parsed.bad]: 'Use a duration like 30m, 4h or 1h 30m.' },
				message: 'Some targets are not durations.'
			});
		}
		const id = formString(form, 'id');
		const version = formString(form, 'version');
		const body = {
			name: formString(form, 'name'),
			description: formString(form, 'description'),
			hoursSource: formString(form, 'hoursSource') || 'client_site',
			hours: hoursFromForm(form),
			timezone: formString(form, 'timezone') || null,
			isDefault: form.get('isDefault') === 'on',
			targets: parsed.targets,
			...(version ? { version: Number(version) } : {})
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/servicedesk/sla-policies/{id}', {
					params: { path: { id } },
					body
				})
			: await api(event).POST('/api/v1/servicedesk/sla-policies', { body });
		if (response.status !== 200 && response.status !== 201) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the policy.')
			});
		}
		return { saved: true };
	},
	archive: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/servicedesk/sla-policies/{id}',
			{ params: { path: { id } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				errors: {},
				message: problemMessage(problem, 'Could not archive the policy.')
			});
		}
		return { archived: true };
	}
};
