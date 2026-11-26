import { error, fail } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { scoreFrom } from '#lib/server/csat.ts';
import type { Actions, PageServerLoad } from './$types';

// The emailed score links open this page with their score chosen; it's
// recorded when they press Send.
export const load: PageServerLoad = async (event) => {
	const { data } = await api(event)
		.GET('/api/portal/v1/nps/{token}', { params: { path: { token: event.params.token } } })
		.catch(() => ({ data: undefined }));
	if (!data) error(404, 'This link isn’t valid.');
	const asked = event.url.searchParams.get('score');
	const n = asked === null ? NaN : Number(asked);
	return { nps: data, selected: Number.isInteger(n) && n >= 0 && n <= 10 ? n : null };
};

export const actions: Actions = {
	default: async (event) => {
		const answer = scoreFrom(await event.request.formData());
		if (!answer) return fail(400, { message: 'Choose a score from 0 to 10.' });
		const { data, response } = await api(event).POST('/api/portal/v1/nps/{token}', {
			params: { path: { token: event.params.token } },
			body: answer
		});
		if (!data) {
			return fail(response.status, {
				message:
					response.status === 409
						? 'This was answered already, or it’s too late to answer.'
						: 'Your answer couldn’t be sent. Try again.'
			});
		}
		return { answered: true };
	}
};
