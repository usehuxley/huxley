import { error, fail } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { ratingFrom } from '#lib/server/csat.ts';
import type { Actions, PageServerLoad } from './$types';

// The emailed rating links open this page with their rating chosen; it's
// recorded when they press Send, so mail scanners opening links don't answer.
export const load: PageServerLoad = async (event) => {
	const { data } = await api(event)
		.GET('/api/portal/v1/surveys/{token}', { params: { path: { token: event.params.token } } })
		.catch(() => ({ data: undefined }));
	if (!data) error(404, 'This survey link isn’t valid.');
	const asked = Number(event.url.searchParams.get('rating'));
	return {
		survey: data,
		selected: Number.isInteger(asked) && asked >= 1 && asked <= 5 ? asked : null
	};
};

export const actions: Actions = {
	default: async (event) => {
		const answer = ratingFrom(await event.request.formData());
		if (!answer) return fail(400, { message: 'Choose a rating.' });
		const { data, response } = await api(event).POST('/api/portal/v1/surveys/{token}', {
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
