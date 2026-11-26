import { error, fail, redirect } from '@sveltejs/kit';
import { answersFor } from '#lib/dashboards/server.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data: submission } = await api(event).GET('/api/v1/dashboard-submissions/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!submission) error(404, 'There’s no such submission.');
	const [{ results }, audiences] = await Promise.all([
		answersFor(event, submission.snapshot, submission.snapshot.range, null),
		api(event).GET('/api/v1/dashboard-audiences')
	]);
	const { user } = await event.parent();
	return {
		submission,
		results,
		audiences: audiences.data ?? { teams: [], roles: [] },
		canDecide: submission.status === 'pending' && submission.submittedBy !== user.id
	};
};

async function decide(event: Parameters<Actions[string]>[0], how: 'approve' | 'reject') {
	const form = await event.request.formData();
	const comment = formString(form, 'comment') || null;
	const path = { params: { path: { id: event.params.id ?? '' } }, body: { comment } };
	const {
		data,
		error: problem,
		response
	} = how === 'approve'
		? await api(event).POST('/api/v1/dashboard-submissions/{id}/approve', path)
		: await api(event).POST('/api/v1/dashboard-submissions/{id}/reject', path);
	if (!data)
		return fail(response.status, { message: problemMessage(problem, 'Could not save that.') });
	if (how === 'approve') redirect(303, `/dashboards/${data.dashboardId}`);
	return { decided: true };
}

export const actions: Actions = {
	approve: (event) => decide(event, 'approve'),
	reject: (event) => decide(event, 'reject')
};
