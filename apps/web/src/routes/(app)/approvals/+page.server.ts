import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { decide } from '#lib/server/approvals.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/v1/approvals');
	if (!data) error(503, 'Could not load approvals.');
	return { approvals: data };
};

export const actions: Actions = { decide };
