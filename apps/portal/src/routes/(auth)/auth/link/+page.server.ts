import { fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { adoptSession, toCodeStep } from '#lib/server/signin.ts';
import type { Actions, PageServerLoad } from './$types';

// Opening the link doesn't sign in: mail scanners open links too, and the
// link works once. Pressing the button does.
export const load: PageServerLoad = ({ url }) => {
	return { token: url.searchParams.get('token') ?? '' };
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const raw = form.get('token');
		const token = typeof raw === 'string' ? raw : '';
		const { response, data } = await api(event)
			.POST('/api/portal/v1/auth/redeem', { body: { token } })
			.catch(() => ({ response: null, data: undefined }));
		if (response?.status === 202) toCodeStep(event.cookies, data, '/');
		if (!response || response.status !== 200) {
			return fail(response?.status ?? 503, {
				message:
					response?.status === 401
						? 'This link has been used or has expired. Ask for a new one.'
						: 'The portal can’t sign you in right now. Try again soon.'
			});
		}
		if (!adoptSession(event.cookies, response.headers.get('set-cookie'))) {
			return fail(502, { message: 'The portal can’t sign you in right now.' });
		}
		redirect(303, '/');
	}
};
