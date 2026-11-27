import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { decide } from '#lib/server/approvals.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/sales/server.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type QuoteInput = components['schemas']['QuoteInput'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [found, revisions, products, signature, review, comments, users] = await Promise.all([
		client.GET('/api/v1/quotes/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/quotes/{id}/revisions', { params: { path: { id } } }),
		client.GET('/api/v1/catalog/products', { params: { query: { active: true } } }),
		client.GET('/api/v1/quotes/{id}/signature', { params: { path: { id } } }),
		client.GET('/api/v1/quotes/{id}/review', { params: { path: { id } } }),
		client.GET('/api/v1/quotes/{id}/comments', { params: { path: { id } } }),
		client.GET('/api/v1/users', { params: { query: { limit: 200 } } })
	]);
	if (found.response.status === 404) error(404, 'Quote not found.');
	if (!found.data) error(503, 'Could not load the quote.');
	const contacts = await client.GET('/api/v1/orgs/{id}/contacts', {
		params: { path: { id: found.data.orgId }, query: { limit: 200 } }
	});
	const approvalId = review.data?.canDecide ? review.data.approvalId : null;
	const approval = approvalId
		? (await client.GET('/api/v1/approvals/{id}', { params: { path: { id: approvalId } } })).data
		: null;
	const { user } = await event.parent();
	return {
		quote: found.data,
		review: review.data ?? null,
		approval: approval ?? null,
		comments: comments.data ?? [],
		people: (users.data?.items ?? [])
			.filter((u) => u.status === 'active' && u.kind === 'human')
			.map((u) => ({ value: u.id, label: u.displayName })),
		me: user.id,
		signature: signature.data ?? null,
		revisions: (revisions.data ?? []).map((r) => ({
			revision: r.revision,
			sentAt: r.sentAt,
			sentByName: r.sentByName
		})),
		products: (products.data ?? []).map((p) => ({
			id: p.id,
			sku: p.sku,
			name: p.name,
			kind: p.kind,
			billing: p.billing,
			price: p.price ?? p.componentsPrice ?? null,
			cost: p.cost ?? null,
			unit: p.unit
		})),
		contacts: (contacts.data?.items ?? []).map((c) => ({ value: c.id, label: c.name })),
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let doc: QuoteInput;
		try {
			doc = JSON.parse(formString(form, 'doc')) as QuoteInput;
		} catch {
			return fail(400, { message: 'Could not read the quote.', errors: {} });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/quotes/{id}', {
			params: { path: { id: event.params.id } },
			body: doc
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the quote.'),
				errors: fieldErrors(problem)
			});
		}
		return { saved: true };
	},
	send: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/quotes/{id}/send', {
			params: { path: { id: event.params.id } },
			body: {
				version: Number(formString(form, 'version')),
				overrideReview: formString(form, 'override') === 'yes'
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not send it.'),
				errors: {}
			});
		}
		return { sent: true };
	},
	submitReview: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/quotes/{id}/review', {
			params: { path: { id: event.params.id } },
			body: { version: Number(formString(form, 'version')) }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not submit it for review.'),
				errors: {}
			});
		}
		return { submitted: true };
	},
	decideReview: decide,
	comment: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/quotes/{id}/comments', {
			params: { path: { id: event.params.id } },
			body: {
				body: formString(form, 'body'),
				lineId: formString(form, 'lineId') || null,
				parentId: formString(form, 'parentId') || null,
				mentions: form.getAll('mentions').map(String)
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not add the comment.'),
				errors: {}
			});
		}
		return { commented: true };
	},
	resolve: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).POST(
			'/api/v1/quotes/{id}/comments/{comment}/resolve',
			{
				params: { path: { id: event.params.id, comment: formString(form, 'comment') } },
				body: { resolved: formString(form, 'resolved') === 'yes' }
			}
		);
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not change it.'),
				errors: {}
			});
		}
		return { resolved: true };
	},
	decide: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/quotes/{id}/decision', {
			params: { path: { id: event.params.id } },
			body: {
				accept: formString(form, 'accept') === 'yes',
				name: formString(form, 'name'),
				reason: formString(form, 'reason') || null,
				options: form.getAll('options').map(String),
				version: Number(formString(form, 'version'))
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not record the answer.'),
				errors: {}
			});
		}
		return { decided: data.status };
	},
	delete: async (event) => {
		const { response, error: problem } = await api(event).DELETE('/api/v1/quotes/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not delete it.'),
				errors: {}
			});
		}
		redirect(303, '/sales/quotes');
	}
};
