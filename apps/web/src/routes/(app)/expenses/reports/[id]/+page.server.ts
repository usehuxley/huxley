import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { decide } from '#lib/server/approvals.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [report, comments] = await Promise.all([
		client.GET('/api/v1/expense-reports/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/expense-reports/{id}/comments', { params: { path: { id } } })
	]);
	if (report.response.status === 404) error(404, 'Report not found.');
	if (!report.data) error(503, 'Could not load the report.');
	const approval =
		report.data.canDecide && report.data.approvalId
			? (
					await client.GET('/api/v1/approvals/{id}', {
						params: { path: { id: report.data.approvalId } }
					})
				).data
			: null;
	const { user } = await event.parent();
	return {
		report: report.data,
		comments: comments.data ?? [],
		approval: approval ?? null,
		me: user.id
	};
};

export const actions: Actions = {
	decideReview: decide,
	comment: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/expense-reports/{id}/comments', {
			params: { path: { id: event.params.id } },
			body: { body: formString(form, 'body'), entryId: formString(form, 'entryId') || null }
		});
		if (!data)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not add the comment.')
			});
		return { message: 'Comment added.' };
	}
};
