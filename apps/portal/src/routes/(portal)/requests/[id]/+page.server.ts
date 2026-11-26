import { error, fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/portal/v1/catalog/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!data) error(404, 'There’s no such request form.');
	return { form: data };
};

export const actions: Actions = {
	default: async (event) => {
		const raw = (await event.request.formData()).get('answers');
		let answers: Record<string, unknown>;
		try {
			answers = JSON.parse(typeof raw === 'string' && raw ? raw : '{}') as Record<string, unknown>;
		} catch {
			return fail(400, { message: 'Your answers couldn’t be read.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/portal/v1/catalog/{id}/requests', {
			params: { path: { id: event.params.id } },
			body: { answers }
		});
		if (!data) {
			const errors: Record<string, string> = {};
			const list = (problem as { errors?: { pointer?: string; message?: string }[] } | undefined)
				?.errors;
			for (const e of list ?? []) {
				const name = e.pointer?.replace(/^\/?(answers\/)?/, '');
				if (name && e.message) errors[name] = e.message;
			}
			return fail(response.status, {
				message:
					response.status === 422
						? 'Check the highlighted answers.'
						: 'Your request couldn’t be sent. Try again.',
				errors
			});
		}
		redirect(303, '/requests?sent=1');
	}
};
