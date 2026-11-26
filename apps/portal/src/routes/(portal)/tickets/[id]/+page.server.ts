import { error, fail } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { ratingFrom } from '#lib/server/csat.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/portal/v1/tickets/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (response.status === 404) error(404, 'Ticket not found.');
	if (!data) error(503, 'Could not load the ticket.');
	return { ticket: data, raised: event.url.searchParams.has('raised') };
};

export const actions: Actions = {
	reply: async (event) => {
		const raw = (await event.request.formData()).get('body');
		const body = typeof raw === 'string' ? raw.trim() : '';
		if (!body) return fail(400, { message: 'Write a reply.' });
		const { response, error: problem } = await api(event).POST(
			'/api/portal/v1/tickets/{id}/replies',
			{ params: { path: { id: event.params.id } }, body: { body } }
		);
		if (response.status !== 201) {
			const detail =
				(problem as { errors?: { message: string }[] } | undefined)?.errors?.[0]?.message ??
				'Your reply couldn’t be sent.';
			return fail(response.status, { message: detail });
		}
		return { sent: true };
	},
	rate: async (event) => {
		const answer = ratingFrom(await event.request.formData());
		if (!answer) return fail(400, { rateMessage: 'Choose a rating.' });
		const { data, response } = await api(event).POST('/api/portal/v1/tickets/{id}/rating', {
			params: { path: { id: event.params.id } },
			body: answer
		});
		if (!data) {
			return fail(response.status, {
				rateMessage:
					response.status === 409
						? 'This was answered already, or it’s too late to answer.'
						: 'Your answer couldn’t be sent.'
			});
		}
		return { rated: true };
	}
};
