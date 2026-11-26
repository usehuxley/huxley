import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [overview, integrations, orgs] = await Promise.all([
		client.GET('/api/v1/workforce/calendars'),
		client.GET('/api/v1/integrations'),
		client.GET('/api/v1/orgs')
	]);
	if (overview.response.status === 404) error(404, 'The workforce module is turned off.');
	if (!overview.data) error(overview.response.status, 'Could not load calendars.');
	// Microsoft Graph connections, wherever they are.
	const graph = (integrations.data ?? []).filter((i) => i.key === 'msgraph');
	const lists = await Promise.all(
		graph.map((i) =>
			client.GET('/api/v1/connections', { params: { query: { integrationId: i.id } } })
		)
	);
	const orgName = new Map((orgs.data ?? []).map((o) => [o.id, o.name]));
	const connections = lists
		.flatMap((l) => l.data ?? [])
		.map((c) => ({
			value: `${c.orgId}|${c.key}`,
			label: `${c.name} (${orgName.get(c.orgId) ?? 'an organization'})`
		}));
	return { ...overview.data, connections };
};

export const actions: Actions = {
	settings: async (event) => {
		const form = await event.request.formData();
		const [orgId, connection] = formString(form, 'connection').split('|');
		const { error: problem, response } = await api(event).PUT(
			'/api/v1/workforce/calendars/settings',
			{
				body: {
					provider: 'm365',
					orgId: orgId ?? '',
					connection: connection ?? '',
					keepSubjects: form.get('keepSubjects') === 'on'
				}
			}
		);
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		return { saved: 'Microsoft 365 is set up.' };
	},
	clear: async (event) => {
		await api(event).DELETE('/api/v1/workforce/calendars/settings');
		return { saved: 'Microsoft 365 is no longer read.' };
	},
	person: async (event) => {
		const form = await event.request.formData();
		const feed = formString(form, 'icsUrl');
		const { error: problem, response } = await api(event).PUT(
			'/api/v1/workforce/calendars/people/{user_id}',
			{
				params: { path: { user_id: formString(form, 'userId') } },
				body: {
					source: formString(form, 'source'),
					calendar: formString(form, 'calendar') || null,
					icsUrl: feed || null
				}
			}
		);
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		return { saved: 'Saved.' };
	}
};
