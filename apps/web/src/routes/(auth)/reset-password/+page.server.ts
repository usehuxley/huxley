import { fail, redirect } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = ({ url }) => ({ token: url.searchParams.get('token') ?? '' });

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const token = formString(form, 'token');
		const newPassword = formString(form, 'newPassword');
		if (newPassword !== formString(form, 'confirmPassword')) {
			return fail(400, { message: 'The passwords do not match.' });
		}
		const { response, error } = await api(event).POST('/api/v1/auth/password-reset/confirm', {
			body: { token, newPassword }
		});
		if (response.status === 204) redirect(303, '/login?reset=done');
		const field = isApiProblem(error) ? error.errors?.[0]?.message : undefined;
		return fail(response.status, {
			message:
				field ??
				(response.status === 400
					? 'This link is invalid or has expired. Ask for a new one.'
					: 'Could not set your password. Try again.')
		});
	}
};
