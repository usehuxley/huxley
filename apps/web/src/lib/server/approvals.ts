import { fail, type RequestEvent } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

/** The decide form action shared by the approval pages. */
export async function decide(event: RequestEvent) {
	const form = await event.request.formData();
	const decision = formString(form, 'decision') === 'reject' ? 'reject' : 'approve';
	const comment = formString(form, 'comment').trim();
	const { data, error, response } = await api(event).POST('/api/v1/approvals/{id}/decision', {
		params: { path: { id: formString(form, 'id') } },
		body: { decision, comment: comment || null }
	});
	if (!data) {
		return fail(response.status, { message: problemMessage(error, 'Could not record it.') });
	}
	return { message: decision === 'approve' ? 'Approved.' : 'Rejected.' };
}
