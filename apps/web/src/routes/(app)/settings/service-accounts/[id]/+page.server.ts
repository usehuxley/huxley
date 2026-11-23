import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { parseTokenForm } from '#lib/server/tokens.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [account, tokens, permissions] = await Promise.all([
		client.GET('/api/v1/service-accounts/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/service-accounts/{id}/tokens', { params: { path: { id } } }),
		client.GET('/api/v1/permissions')
	]);
	if (account.response.status === 404) error(404, 'Service account not found.');
	if (!account.data || !tokens.data) error(503, 'Could not load the service account.');
	return { account: account.data, tokens: tokens.data, permissions: permissions.data ?? [] };
};

export const actions: Actions = {
	update: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PATCH('/api/v1/service-accounts/{id}', {
			params: { path: { id: event.params.id } },
			body: { name: formString(form, 'name'), description: formString(form, 'description') }
		});
		if (response.status !== 200) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save.') });
		}
		return { message: 'Saved.' };
	},

	status: async (event) => {
		const form = await event.request.formData();
		const status = formString(form, 'status') === 'disabled' ? 'disabled' : 'active';
		const { response } = await api(event).PATCH('/api/v1/service-accounts/{id}', {
			params: { path: { id: event.params.id } },
			body: { status }
		});
		if (response.status !== 200)
			return fail(response.status, { message: 'Could not change status.' });
		return { message: status === 'disabled' ? 'Disabled: its tokens stop working.' : 'Enabled.' };
	},

	createToken: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/service-accounts/{id}/tokens', {
			params: { path: { id: event.params.id } },
			body: parseTokenForm(form)
		});
		if (!data) {
			return fail(response.status, {
				tokens: { error: problemMessage(problem, 'Could not create the token.') }
			});
		}
		return { tokens: { created: { name: data.name, secret: data.tokenSecret } } };
	},

	revokeToken: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).DELETE(
			'/api/v1/service-accounts/{id}/tokens/{token_id}',
			{
				params: { path: { id: event.params.id, token_id: formString(form, 'id') } }
			}
		);
		if (response.status !== 204) {
			return fail(response.status, { tokens: { error: 'Could not revoke the token.' } });
		}
		return { tokens: { revoked: true } };
	}
};
