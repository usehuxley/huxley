import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	event.depends('app:notifications');
	const client = api(event);
	const cursor = event.url.searchParams.get('cursor') ?? undefined;
	const [feed, preferences] = await Promise.all([
		client.GET('/api/v1/notifications', { params: { query: { cursor, limit: 50 } } }),
		client.GET('/api/v1/notifications/preferences')
	]);
	if (!feed.data || !preferences.data) error(503, 'Could not load your notifications.');
	return { feed: feed.data, preferences: preferences.data };
};

export const actions: Actions = {
	read: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { response } = await api(event).POST('/api/v1/notifications/{id}/read', {
			params: { path: { id } }
		});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not mark it read.' });
		return {};
	},

	readAll: async (event) => {
		await api(event).POST('/api/v1/notifications/read-all');
		return {};
	},

	preference: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).PUT('/api/v1/notifications/preferences/{kind}', {
			params: { path: { kind: formString(form, 'kind') } },
			body: { inApp: form.get('inApp') === 'on', email: form.get('email') === 'on' }
		});
		if (response.status !== 204) return fail(response.status, { message: 'Could not save.' });
		return { message: 'Saved.' };
	}
};
