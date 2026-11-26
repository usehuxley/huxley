import { fail } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { Actions } from './$types';

export const actions: Actions = {
	password: async (event) => {
		const form = await event.request.formData();
		const raw = form.get('password');
		const password = typeof raw === 'string' ? raw : '';
		const again = form.get('again');
		if (password !== again) return fail(400, { message: 'The passwords don’t match.' });
		const { response, error } = await api(event).PUT('/api/portal/v1/me/password', {
			body: { password }
		});
		if (response.status !== 204) {
			const detail =
				(error as { errors?: { message: string }[] } | undefined)?.errors?.[0]?.message ??
				'Could not set it.';
			return fail(response.status, { message: detail });
		}
		return { done: true };
	}
};
