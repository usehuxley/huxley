import { fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

// Verifying takes a button press, not just opening the link: mail scanners
// open links automatically, and that shouldn't use up the token.
export const load: PageServerLoad = ({ url }) => ({ token: url.searchParams.get('token') ?? '' });

export const actions: Actions = {
	default: async (event) => {
		const token = formString(await event.request.formData(), 'token');
		const { response } = await api(event).POST('/api/v1/auth/email/verify', { body: { token } });
		if (response.status !== 204) {
			return fail(response.status, { message: 'This link is invalid or has expired.' });
		}
		return { verified: true };
	}
};
