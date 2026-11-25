import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad, RequestEvent } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/kb/articles/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!data) error(response.status === 404 ? 404 : 503, 'Could not load the article.');
	return { article: data };
};

type Step = 'submit' | 'publish' | 'reject' | 'archive' | 'restore';
const DONE: Record<Step, string> = {
	submit: 'Submitted for review.',
	publish: 'Published.',
	reject: 'Sent back to the author.',
	archive: 'Archived.',
	restore: 'Restored.'
};

async function step(event: RequestEvent, name: Step) {
	const form = await event.request.formData();
	const comment = formString(form, 'comment').trim();
	const params = { path: { id: event.params.id } };
	const client = api(event);
	const result =
		name === 'submit'
			? await client.POST('/api/v1/kb/articles/{id}/submit', { params })
			: name === 'publish'
				? await client.POST('/api/v1/kb/articles/{id}/publish', {
						params,
						body: { comment: comment || null }
					})
				: name === 'reject'
					? await client.POST('/api/v1/kb/articles/{id}/reject', { params, body: { comment } })
					: name === 'archive'
						? await client.POST('/api/v1/kb/articles/{id}/archive', { params })
						: await client.POST('/api/v1/kb/articles/{id}/restore', { params });
	if (!result.data) {
		return fail(result.response.status, {
			message: problemMessage(result.error, 'That did not work.')
		});
	}
	return { message: DONE[name] };
}

export const actions: Actions = {
	submit: (event) => step(event, 'submit'),
	publish: (event) => step(event, 'publish'),
	reject: (event) => step(event, 'reject'),
	archive: (event) => step(event, 'archive'),
	restore: (event) => step(event, 'restore')
};
