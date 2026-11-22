import { error, redirect } from '@sveltejs/kit';
import { visibleSections } from '#lib/permissions.ts';
import type { PageServerLoad } from './$types';

/** /settings goes to the first section the user can see. */
export const load: PageServerLoad = async ({ parent }) => {
	const { user } = await parent();
	const first = visibleSections(user.grantedPermissions)[0];
	if (!first) error(403, 'You do not have access to any settings.');
	redirect(307, `/settings/${first.key}`);
};
