import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/platform/audit-streams');
	if (response.status === 403) error(403, 'You can’t manage audit streaming.');
	if (!data) error(503, 'Could not load the audit streams.');
	return { streams: data };
};

function body(form: FormData) {
	const kind = formString(form, 'kind') === 'splunk_hec' ? 'splunk_hec' : 'webhook';
	const secret = formString(form, 'secret');
	return {
		name: formString(form, 'name'),
		kind,
		url: formString(form, 'url'),
		headerName: kind === 'webhook' ? formString(form, 'headerName') || null : null,
		secret: secret === '' ? null : secret,
		enabled: form.get('enabled') === 'on',
		fromStart: form.get('fromStart') === 'on'
	} as const;
}

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = id
			? await client.PUT('/api/v1/platform/audit-streams/{id}', {
					params: { path: { id } },
					body: body(form)
				})
			: await client.POST('/api/v1/platform/audit-streams', { body: body(form) });
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the stream.')
			});
		}
		return { saved: true, message: id ? 'Stream saved.' : 'Stream added.' };
	},
	test: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/platform/audit-streams/{id}/test', {
			params: { path: { id } }
		});
		if (!data)
			return fail(response.status, { message: problemMessage(problem, 'The test failed.') });
		if (!data.ok) return fail(502, { message: data.error ?? 'The destination refused the test.' });
		return { message: 'The test event was delivered.' };
	},
	delete: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/platform/audit-streams/{id}',
			{ params: { path: { id } } }
		);
		if (response.status !== 204)
			return fail(response.status, { message: problemMessage(problem, 'Could not remove it.') });
		return { message: 'Stream removed.' };
	}
};
