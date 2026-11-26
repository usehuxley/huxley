import { error, fail } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/portal/v1/approvals/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!data) error(404, 'There’s no such approval.');
	return { approval: data };
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const raw = form.get('comment');
		const comment = typeof raw === 'string' && raw.trim() ? raw.trim() : undefined;
		const approve = form.get('decision') === 'approve';
		const { data, response } = await api(event).POST('/api/portal/v1/approvals/{id}/decision', {
			params: { path: { id: event.params.id } },
			body: { approve, comment }
		});
		if (!data) {
			return fail(response.status, {
				message:
					response.status === 409
						? 'It was already decided, or it expired.'
						: 'Your decision couldn’t be saved. Try again.'
			});
		}
		return { decided: data.status };
	}
};
