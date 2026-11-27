import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { RequestEvent } from '@sveltejs/kit';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [opp, scope, templates] = await Promise.all([
		client.GET('/api/v1/opportunities/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/opportunities/{id}/scope', { params: { path: { id } } }),
		client.GET('/api/v1/scope-templates')
	]);
	if (!opp.data) error(opp.response.status === 404 ? 404 : 503, 'Opportunity not found.');
	const { user } = await event.parent();
	const [estimate, catalog] = scope.data
		? await Promise.all([
				client.GET('/api/v1/scopes/{id}/estimate', { params: { path: { id: scope.data.id } } }),
				client.GET('/api/v1/catalog/products', { params: { query: { active: true } } })
			])
		: [null, null];
	const all = catalog?.data ?? [];
	return {
		opportunity: { id: opp.data.id, name: opp.data.name, orgName: opp.data.orgName },
		scope: scope.data ?? null,
		estimate: estimate?.data ?? null,
		roles: all
			.filter((p) => p.kind === 'service' && p.unit.toLowerCase().startsWith('hour'))
			.map((p) => ({ value: p.id, label: p.name })),
		products: all
			.filter((p) => p.kind === 'product')
			.map((p) => ({ value: p.id, label: `${p.name} · ${p.sku}` })),
		templates: templates.data ?? [],
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};

type Said = { message: string };
const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

async function scopeId(event: RequestEvent) {
	const { data } = await api(event).GET('/api/v1/opportunities/{id}/scope', {
		params: { path: { id: event.params.id! } }
	});
	return data?.id ?? '';
}

export const actions: Actions = {
	open: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/opportunities/{id}/scope', {
			params: { path: { id: event.params.id } },
			body: { template: formString(form, 'template') }
		});
		if (!data) return failed(response.status, problem, 'Could not start scoping.');
		redirect(303, `/sales/opportunities/${event.params.id}/scope`);
	},
	source: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scopes/{id}/sources', {
			params: { path: { id } },
			body: {
				kind: formString(form, 'kind') || 'note',
				title: formString(form, 'title'),
				body: formString(form, 'body')
			}
		});
		if (!data) return failed(response.status, problem, 'Could not add it.');
		return { message: 'Added to discovery.' } satisfies Said;
	},
	removeSource: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).DELETE('/api/v1/scopes/{id}/sources/{source}', {
			params: { path: { id, source: formString(form, 'source') } }
		});
		if (!data) return failed(response.status, problem, 'Could not remove it.');
		return { message: 'Removed.' } satisfies Said;
	},
	section: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/scopes/{id}/sections/{key}', {
			params: { path: { id, key: formString(form, 'key') } },
			body: { body: formString(form, 'body') }
		});
		if (!data) return failed(response.status, problem, 'Could not save the section.');
		return { message: 'Saved.' } satisfies Said;
	},
	question: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scopes/{id}/questions', {
			params: { path: { id } },
			body: {
				question: formString(form, 'question'),
				sectionKey: formString(form, 'sectionKey') || null
			}
		});
		if (!data) return failed(response.status, problem, 'Could not add it.');
		return { message: 'Question added.' } satisfies Said;
	},
	answer: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scopes/{id}/questions/{question}', {
			params: { path: { id, question: formString(form, 'question') } },
			body: {
				answer: formString(form, 'answer') || null,
				resolved: formString(form, 'resolved') === 'yes'
			}
		});
		if (!data) return failed(response.status, problem, 'Could not save the answer.');
		return { message: 'Saved.' } satisfies Said;
	},
	draft: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const cancel = formString(form, 'cancel') === 'yes';
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = cancel
			? await client.DELETE('/api/v1/scopes/{id}/draft', { params: { path: { id } } })
			: await client.POST('/api/v1/scopes/{id}/draft', { params: { path: { id } } });
		if (!data) return failed(response.status, problem, 'Could not ask for a draft.');
		return {
			message: cancel ? 'Stopped waiting.' : 'Drafting: this takes a minute.'
		} satisfies Said;
	},
	approve: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scopes/{id}/approve', {
			params: { path: { id } },
			body: { version: Number(formString(form, 'version')) }
		});
		if (!data) return failed(response.status, problem, 'Could not approve it.');
		return { message: `Approved as version ${data.versions[0]?.number ?? ''}.` } satisfies Said;
	},
	tasks: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		let doc: components['schemas']['ScopeTaskInput'][];
		try {
			doc = JSON.parse(formString(form, 'doc')) as components['schemas']['ScopeTaskInput'][];
		} catch {
			return fail(400, { message: 'Could not read the tasks.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/scopes/{id}/estimate/tasks', {
			params: { path: { id } },
			body: doc
		});
		if (!data) return failed(response.status, problem, 'Could not save the work breakdown.');
		return { message: 'Work breakdown saved.' } satisfies Said;
	},
	items: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		let doc: components['schemas']['ScopeItemInput'][];
		try {
			doc = JSON.parse(formString(form, 'doc')) as components['schemas']['ScopeItemInput'][];
		} catch {
			return fail(400, { message: 'Could not read the materials.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/scopes/{id}/estimate/items', {
			params: { path: { id } },
			body: doc
		});
		if (!data) return failed(response.status, problem, 'Could not save the materials.');
		return { message: 'Materials saved.' } satisfies Said;
	},
	risk: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const risk =
			(['low', 'medium', 'high'] as const).find((r) => r === formString(form, 'risk')) ?? 'medium';
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/scopes/{id}/estimate/risk', {
			params: { path: { id } },
			body: { risk }
		});
		if (!data) return failed(response.status, problem, 'Could not set the risk.');
		return { message: `Contingency ${data.contingencyPct}%.` } satisfies Said;
	},
	estimate: async (event) => {
		const form = await event.request.formData();
		const id = await scopeId(event);
		const cancel = formString(form, 'cancel') === 'yes';
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = cancel
			? await client.DELETE('/api/v1/scopes/{id}/estimate', { params: { path: { id } } })
			: await client.POST('/api/v1/scopes/{id}/estimate', { params: { path: { id } } });
		if (!data) return failed(response.status, problem, 'Could not ask for an estimate.');
		return {
			message: cancel ? 'Stopped waiting.' : 'Estimating: this takes a minute.'
		} satisfies Said;
	},
	assemble: async (event) => {
		const id = await scopeId(event);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scopes/{id}/quote', {
			params: { path: { id } }
		});
		if (!data) return failed(response.status, problem, 'Could not build the quote.');
		redirect(303, `/sales/quotes/${data.quoteId}`);
	}
};
