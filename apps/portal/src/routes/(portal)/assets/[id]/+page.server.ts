import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/portal/v1/devices/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (!data) error(404, 'There’s no such device.');
	return { device: data };
};
