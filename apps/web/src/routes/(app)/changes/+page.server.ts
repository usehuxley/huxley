import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const from = new Date();
	from.setHours(0, 0, 0, 0);
	const to = new Date(from.getTime() + 28 * 86_400_000);
	const { user } = await event.parent();
	const [calendar, orgs] = await Promise.all([
		client.GET('/api/v1/changes/calendar', {
			params: { query: { from: from.toISOString(), to: to.toISOString() } }
		}),
		client.GET('/api/v1/orgs')
	]);
	if (calendar.response.status === 404) error(404, 'The service desk is turned off.');
	if (!calendar.data) error(calendar.response.status, 'Could not load the change calendar.');
	return {
		calendar: calendar.data,
		from: from.toISOString(),
		orgs: (orgs.data ?? []).map((o) => ({ value: o.id, label: o.name })),
		canFreeze: user.grantedPermissions.includes('servicedesk:configure')
	};
};

export const actions: Actions = {
	freeze: async (event) => {
		const form = await event.request.formData();
		const toIso = (v: string) => (v ? new Date(v).toISOString() : '');
		const { error: problem, response } = await api(event).POST('/api/v1/change-freezes', {
			body: {
				orgId: formString(form, 'orgId'),
				name: formString(form, 'name'),
				startsAt: toIso(formString(form, 'startsAt')),
				endsAt: toIso(formString(form, 'endsAt'))
			}
		});
		if (response.status !== 201) {
			return fail(response.status, {
				freeze: problemMessage(problem, 'Could not add the freeze.')
			});
		}
		return { freeze: 'Freeze added.' };
	},
	unfreeze: async (event) => {
		const form = await event.request.formData();
		await api(event).DELETE('/api/v1/change-freezes/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		return { freeze: 'Freeze removed.' };
	}
};
