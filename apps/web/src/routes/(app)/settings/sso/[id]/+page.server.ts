import { error, fail, redirect } from '@sveltejs/kit';
import type { components } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors, parseProviderForm } from '#lib/server/sso.ts';
import type { Actions, PageServerLoad, RequestEvent } from './$types';

type RoleMapping = components['schemas']['RoleMapping'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [provider, roles, orgs, scim] = await Promise.all([
		client.GET('/api/v1/security/sso/providers/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/roles'),
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/security/sso/providers/{id}/scim', { params: { path: { id } } })
	]);
	if (provider.response.status === 404) error(404, 'Identity provider not found.');
	if (!provider.data || !roles.data || !orgs.data) error(503, 'Could not load the provider.');
	return {
		provider: provider.data,
		// Owner is granted by hand only, never from a claim.
		roles: roles.data.filter((r) => r.key !== 'owner'),
		orgs: orgs.data.filter((o) => o.status === 'active'),
		scim: scim.data ?? null
	};
};

async function saveMappings(event: RequestEvent, mappings: RoleMapping[], message: string) {
	const { response, error: problem } = await api(event).PUT(
		'/api/v1/security/sso/providers/{id}/role-mappings',
		{ params: { path: { id: event.params.id } }, body: mappings }
	);
	if (response.status !== 204) {
		return fail(response.status, {
			mappingError: problemMessage(problem, 'Could not save the role mappings.')
		});
	}
	return { message };
}

async function currentMappings(event: RequestEvent): Promise<RoleMapping[]> {
	const { data } = await api(event).GET('/api/v1/security/sso/providers/{id}', {
		params: { path: { id: event.params.id } }
	});
	return data?.roleMappings ?? [];
}

export const actions: Actions = {
	update: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/security/sso/providers/{id}', {
			params: { path: { id: event.params.id } },
			body: parseProviderForm(form)
		});
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the provider.')
			});
		}
		return { message: 'Saved.' };
	},

	addMapping: async (event) => {
		const form = await event.request.formData();
		const mapping: RoleMapping = {
			claimValue: formString(form, 'claimValue').trim(),
			roleId: formString(form, 'roleId'),
			orgId: formString(form, 'orgId'),
			inherit: form.get('inherit') === 'on'
		};
		return saveMappings(event, [...(await currentMappings(event)), mapping], 'Mapping added.');
	},

	removeMapping: async (event) => {
		const form = await event.request.formData();
		const index = Number(formString(form, 'index'));
		const mappings = (await currentMappings(event)).filter((_, i) => i !== index);
		return saveMappings(event, mappings, 'Mapping removed.');
	},

	scimToken: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/security/sso/providers/{id}/scim/token', {
			params: { path: { id: event.params.id } }
		});
		if (!data)
			return fail(response.status, { message: problemMessage(problem, 'Could not make a token.') });
		// Shown once, from this response only.
		return { scimToken: data.token, scimUrl: data.url ?? null };
	},

	scimDisable: async (event) => {
		const { response } = await api(event).DELETE('/api/v1/security/sso/providers/{id}/scim', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not turn it off.' });
		return { message: 'Provisioning is off.' };
	},

	delete: async (event) => {
		const { response } = await api(event).DELETE('/api/v1/security/sso/providers/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204) return fail(response.status, { message: 'Could not delete.' });
		redirect(303, '/settings/sso');
	}
};
