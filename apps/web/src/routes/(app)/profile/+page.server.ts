import { error, fail } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/v1/auth/sessions');
	if (!data) error(503, 'Could not load your sessions.');
	return { sessions: data };
};

export const actions: Actions = {
	changePassword: async (event) => {
		const form = await event.request.formData();
		const currentPassword = formString(form, 'currentPassword');
		const newPassword = formString(form, 'newPassword');
		const confirmPassword = formString(form, 'confirmPassword');
		if (newPassword !== confirmPassword) {
			return fail(400, {
				password: { errors: { confirmPassword: 'The new passwords do not match.' } }
			});
		}

		const { response, error: problem } = await api(event).POST('/api/v1/auth/password', {
			body: { currentPassword, newPassword }
		});
		if (response.status === 204) {
			return { password: { changed: true } };
		}
		const errors: Record<string, string> = {};
		if (isApiProblem(problem)) {
			for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
		}
		if (Object.keys(errors).length === 0)
			errors.form = 'Could not change your password. Try again.';
		return fail(response.status, { password: { errors } });
	},

	revokeSession: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const { response } = await api(event).DELETE('/api/v1/auth/sessions/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return fail(response.status, { revoke: { failed: true } });
		return { revoke: { done: true } };
	}
};
