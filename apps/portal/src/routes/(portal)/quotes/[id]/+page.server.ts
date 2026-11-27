import { error, fail } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/portal/v1/quotes/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!data) error(404, 'There’s no such quote.');
	return { quote: data };
};

const text = (form: FormData, name: string) => {
	const v = form.get(name);
	return typeof v === 'string' ? v.trim() : '';
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const accept = text(form, 'decision') === 'accept';
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/portal/v1/quotes/{id}/answer', {
			params: { path: { id: event.params.id } },
			body: {
				accept,
				name: text(form, 'name'),
				signature: text(form, 'signature') || null,
				options: form.getAll('options').map(String),
				reason: text(form, 'reason') || null,
				agree: form.get('agree') === 'on',
				revision: Number(text(form, 'revision'))
			}
		});
		if (!data) {
			const detail = isApiProblem(problem)
				? (problem.errors?.[0]?.message ?? problem.detail ?? undefined)
				: undefined;
			return fail(response.status, {
				message: detail
					? `${detail.charAt(0).toUpperCase()}${detail.slice(1)}.`
					: 'Your answer couldn’t be saved. Try again.'
			});
		}
		return { answered: data.status };
	}
};
