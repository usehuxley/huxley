import { api } from '#lib/server/api.ts';
import { fail } from '@sveltejs/kit';
import { scoreFrom } from '#lib/server/csat.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const [tickets, approvals, nps] = await Promise.all([
		api(event)
			.GET('/api/portal/v1/tickets', { params: { query: { status: 'open' } } })
			.catch(() => ({ data: undefined })),
		api(event)
			.GET('/api/portal/v1/approvals')
			.catch(() => ({ data: undefined })),
		api(event)
			.GET('/api/portal/v1/me/nps')
			.catch(() => ({ data: undefined }))
	]);
	return {
		open: (tickets.data ?? []).slice(0, 5),
		waiting: (approvals.data ?? []).filter((a) => a.status === 'pending').length,
		nps: nps.data ?? null
	};
};

export const actions: Actions = {
	nps: async (event) => {
		const answer = scoreFrom(await event.request.formData());
		if (!answer) return fail(400, { npsMessage: 'Choose a score from 0 to 10.' });
		const { data, response } = await api(event).POST('/api/portal/v1/me/nps', { body: answer });
		if (!data) return fail(response.status, { npsMessage: 'Your answer couldn’t be sent.' });
		return { npsAnswered: true };
	}
};
