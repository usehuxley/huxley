import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/projects');
	if (response.status === 404) error(404, 'Projects are turned off.');
	if (!data) error(response.status, 'Could not load projects.');
	const { user } = await event.parent();
	return { projects: data, canManage: user.grantedPermissions.includes('project:manage') };
};
