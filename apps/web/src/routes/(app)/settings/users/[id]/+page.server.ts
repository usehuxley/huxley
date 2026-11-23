import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const orgParam = event.url.searchParams.get('org');
	const [person, assignments, roles, orgs, teams, effective] = await Promise.all([
		client.GET('/api/v1/users/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/users/{id}/assignments', { params: { path: { id } } }),
		client.GET('/api/v1/roles'),
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/teams/of-user/{user_id}', { params: { path: { user_id: id } } }),
		client.GET('/api/v1/users/{id}/effective-permissions', {
			params: { path: { id }, query: { orgId: orgParam ?? undefined } }
		})
	]);
	if (person.response.status === 404) error(404, 'User not found.');
	if (!person.data || !assignments.data) error(503, 'Could not load this user.');
	return {
		person: person.data,
		assignments: assignments.data,
		// Users who can't assign roles may still view; the API decides.
		roles: roles.data ?? [],
		orgs: (orgs.data ?? []).filter((o) => o.status === 'active'),
		teams: teams.data ?? [],
		effective: effective.data ?? null,
		effectiveOrg: orgParam
	};
};

export const actions: Actions = {
	status: async (event) => {
		const form = await event.request.formData();
		const status = formString(form, 'status') === 'disabled' ? 'disabled' : 'active';
		const { error: problem } = await api(event).PATCH('/api/v1/users/{id}', {
			params: { path: { id: event.params.id } },
			body: { status }
		});
		if (problem)
			return fail(400, { message: problemMessage(problem, 'Could not update the user.') });
		return { message: status === 'disabled' ? 'User disabled and signed out.' : 'User enabled.' };
	},

	resetMfa: async (event) => {
		const { error: problem } = await api(event).DELETE('/api/v1/users/{id}/mfa', {
			params: { path: { id: event.params.id } }
		});
		if (problem)
			return fail(400, {
				message: problemMessage(problem, 'Could not reset two-factor authentication.')
			});
		return { message: 'Two-factor authentication removed. The user was signed out.' };
	},

	assign: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).POST('/api/v1/role-assignments', {
			body: {
				userId: event.params.id,
				roleId: formString(form, 'roleId'),
				orgId: formString(form, 'orgId'),
				inherit: form.get('inherit') === 'on'
			}
		});
		if (problem)
			return fail(400, { message: problemMessage(problem, 'Could not assign the role.') });
		return { message: 'Role assigned.' };
	},

	unassign: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).DELETE('/api/v1/role-assignments/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (problem)
			return fail(400, { message: problemMessage(problem, 'Could not remove the role.') });
		return { message: 'Role removed.' };
	}
};
