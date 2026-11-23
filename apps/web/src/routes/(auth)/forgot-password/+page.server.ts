import { fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import type { Actions } from './$types';

export const actions: Actions = {
	default: async (event) => {
		const email = formString(await event.request.formData(), 'email').trim();
		if (!email) return fail(400, { message: 'Enter your email address.' });
		const { response } = await api(event)
			.POST('/api/v1/auth/password-reset', { body: { email }, signal: AbortSignal.timeout(10000) })
			.catch(() => ({ response: { status: 503 } }));
		if (response.status === 429) {
			return fail(429, { message: 'Too many attempts. Wait a minute and try again.' });
		}
		if (response.status !== 202) {
			return fail(503, { message: "Password reset isn't available right now." });
		}
		// The same answer whether or not the account exists.
		return { sent: true };
	}
};
