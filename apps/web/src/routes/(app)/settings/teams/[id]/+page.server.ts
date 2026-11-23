import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [team, users, roles, orgs] = await Promise.all([
		client.GET('/api/v1/teams/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/users', { params: { query: { limit: 200 } } }),
		client.GET('/api/v1/roles'),
		client.GET('/api/v1/orgs')
	]);
	if (team.response.status === 404) error(404, 'Team not found.');
	if (!team.data) error(503, 'Could not load the team.');
	return {
		team: team.data,
		users: users.data?.items ?? [],
		// Owner goes to individual people only.
		roles: (roles.data ?? []).filter((r) => r.key !== 'owner'),
		orgs: (orgs.data ?? []).filter((o) => o.status === 'active')
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	update: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT('/api/v1/teams/{id}', {
			params: { path: { id: event.params.id } },
			body: { name: formString(form, 'name'), description: formString(form, 'description') }
		});
		if (response.status !== 200)
			return failed(response.status, problem, 'Could not save the team.');
		return { message: 'Saved.' };
	},

	addMember: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT(
			'/api/v1/teams/{id}/members/{user_id}',
			{
				params: { path: { id: event.params.id, user_id: formString(form, 'userId') } }
			}
		);
		if (response.status !== 204)
			return failed(response.status, problem, 'Could not add the member.');
		return { message: 'Member added.' };
	},

	removeMember: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/teams/{id}/members/{user_id}',
			{ params: { path: { id: event.params.id, user_id: formString(form, 'userId') } } }
		);
		if (response.status !== 204)
			return failed(response.status, problem, 'Could not remove the member.');
		return { message: 'Member removed.' };
	},

	assign: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).POST('/api/v1/role-assignments', {
			body: {
				teamId: event.params.id,
				roleId: formString(form, 'roleId'),
				orgId: formString(form, 'orgId'),
				inherit: form.get('inherit') === 'on'
			}
		});
		if (problem) return failed(400, problem, 'Could not assign the role.');
		return { message: 'Role assigned.' };
	},

	unassign: async (event) => {
		const form = await event.request.formData();
		const { error: problem } = await api(event).DELETE('/api/v1/role-assignments/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (problem) return failed(400, problem, 'Could not remove the role.');
		return { message: 'Role removed.' };
	},

	delete: async (event) => {
		const { response } = await api(event).DELETE('/api/v1/teams/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not delete the team.' });
		redirect(303, '/settings/teams');
	}
};
