import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/timesheets/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (response.status === 404) error(404, 'Timesheet not found.');
	if (!data) error(503, 'Could not load the timesheet.');
	return { sheet: data };
};

export const actions: Actions = {
	decide: async (event) => {
		const form = await event.request.formData();
		const approve = formString(form, 'approve') === 'true';
		const { error: problem, response } = await api(event).POST('/api/v1/timesheets/{id}/decision', {
			params: { path: { id: event.params.id } },
			body: { approve, note: formString(form, 'note') || null }
		});
		if (response.status !== 200) {
			return fail(response.status, { message: problemMessage(problem, 'Could not decide it.') });
		}
		return { done: approve ? 'Approved: its time is locked.' : 'Rejected, with your comment.' };
	},
	reopen: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/timesheets/{id}/reopen', {
			params: { path: { id: event.params.id } },
			body: { note: formString(form, 'note') || null }
		});
		if (response.status !== 200) {
			return fail(response.status, { message: problemMessage(problem, 'Could not reopen it.') });
		}
		return { done: 'Reopened: its time can change again.' };
	}
};
