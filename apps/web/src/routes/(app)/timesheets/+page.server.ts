import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

const STATUSES = ['submitted', 'approved', 'rejected', 'open'];

export const load: PageServerLoad = async (event) => {
	const asked = event.url.searchParams.get('status') ?? 'submitted';
	const status = STATUSES.includes(asked) ? asked : 'submitted';
	const { data, response } = await api(event).GET('/api/v1/timesheets', {
		params: { query: { status } }
	});
	if (response.status === 404) error(404, 'Timesheets are turned off.');
	if (response.status === 403) error(403, 'You don’t review timesheets.');
	if (!data) error(503, 'Could not load the timesheets.');
	const { user } = await event.parent();
	const manages = user.grantedPermissions.includes('workforce:manage');
	const settings = manages ? (await api(event).GET('/api/v1/timesheets/settings')).data : undefined;
	return { timesheets: data, status, settings: settings ?? null };
};

export const actions: Actions = {
	settings: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT('/api/v1/timesheets/settings', {
			body: {
				periodDays: Number(formString(form, 'periodDays')) || 7,
				anchor: formString(form, 'anchor'),
				billApprovedOnly: form.get('billApprovedOnly') === 'on'
			}
		});
		if (response.status !== 200) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save them.') });
		}
		return { done: 'Timesheet settings saved.' };
	}
};
