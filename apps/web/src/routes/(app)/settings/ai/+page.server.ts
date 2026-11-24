import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

const KINDS = ['anthropic', 'openai', 'azure_openai', 'openai_compatible'] as const;
type Kind = (typeof KINDS)[number];

export const load: PageServerLoad = async (event) => {
	const { currentOrg } = await event.parent();
	const client = api(event);
	const orgs = (await client.GET('/api/v1/orgs')).data ?? [];
	const orgId = event.url.searchParams.get('org') ?? currentOrg?.id ?? orgs[0]?.id;
	if (!orgId) error(404, 'No organization to set up.');
	const [provider, calls] = await Promise.all([
		client.GET('/api/v1/ai/provider', { params: { query: { orgId } } }),
		client.GET('/api/v1/ai/calls', { params: { query: { orgId } } })
	]);
	if (provider.response.status === 403 || provider.response.status === 404) {
		error(403, 'You do not have access to AI settings here.');
	}
	if (!provider.data) error(503, 'Could not load the AI settings.');
	return {
		orgId,
		orgs: orgs.map((o) => ({ id: o.id, name: o.name })),
		provider: provider.data,
		calls: calls.data ?? []
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const orgId = formString(form, 'orgId');
		const kindText = formString(form, 'kind');
		const kind: Kind = (KINDS as readonly string[]).includes(kindText)
			? (kindText as Kind)
			: 'anthropic';
		const optional = (name: string) => formString(form, name).trim() || null;
		const budget = formString(form, 'budget').trim();
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/ai/provider', {
			body: {
				orgId,
				provider: {
					kind,
					model: optional('model'),
					baseUrl: optional('baseUrl'),
					deployment: optional('deployment'),
					apiKey: optional('apiKey'),
					monthlyTokenBudget: budget ? Number(budget) : null
				}
			}
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		}
		return { message: 'AI provider saved.' };
	},
	remove: async (event) => {
		const orgId = formString(await event.request.formData(), 'orgId');
		const { response, error: problem } = await api(event).DELETE('/api/v1/ai/provider', {
			params: { query: { orgId } }
		});
		if (!response.ok) {
			return fail(response.status, { message: problemMessage(problem, 'Could not remove it.') });
		}
		return { message: 'AI provider removed.' };
	},
	test: async (event) => {
		const orgId = formString(await event.request.formData(), 'orgId');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/ai/provider/test', {
			params: { query: { orgId } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'The test failed.') });
		}
		return { test: data };
	}
};
