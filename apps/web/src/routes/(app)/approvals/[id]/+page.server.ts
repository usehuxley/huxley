import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { decide } from '#lib/server/approvals.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/approvals/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (response.status === 404 || response.status === 403) error(404, 'Approval not found.');
	if (!data) error(503, 'Could not load the approval.');
	return { approval: data };
};

export const actions: Actions = { decide };
