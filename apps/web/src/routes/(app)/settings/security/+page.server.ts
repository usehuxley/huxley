import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [policy, roles, orgs] = await Promise.all([
		client.GET('/api/v1/security/mfa-policy'),
		client.GET('/api/v1/roles'),
		client.GET('/api/v1/orgs')
	]);
	if (policy.response.status === 403) error(403, 'You cannot manage security policy.');
	if (!policy.data || !roles.data || !orgs.data) error(503, 'Could not load security policy.');
	return {
		policy: policy.data,
		roles: roles.data,
		orgs: orgs.data.filter((o) => o.status === 'active')
	};
};

export const actions: Actions = {
	setRole: async (event) => {
		const form = await event.request.formData();
		const required = formString(form, 'required') === 'true';
		const { response, error: problem } = await api(event).PUT(
			'/api/v1/security/mfa-policy/roles/{id}',
			{ params: { path: { id: formString(form, 'id') } }, body: { required } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not change the policy.')
			});
		}
		return { message: required ? 'Two-factor authentication required.' : 'Requirement removed.' };
	},

	setOrg: async (event) => {
		const form = await event.request.formData();
		const required = formString(form, 'required') === 'true';
		const { response, error: problem } = await api(event).PUT(
			'/api/v1/security/mfa-policy/orgs/{id}',
			{ params: { path: { id: formString(form, 'id') } }, body: { required } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not change the policy.')
			});
		}
		return { message: required ? 'Two-factor authentication required.' : 'Requirement removed.' };
	}
};
