import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { decide } from '#lib/server/approvals.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [{ data, response }, review, comments, users] = await Promise.all([
		client.GET('/api/v1/invoices/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/invoices/{id}/review', { params: { path: { id } } }),
		client.GET('/api/v1/invoices/{id}/comments', { params: { path: { id } } }),
		client.GET('/api/v1/users', { params: { query: { limit: 200 } } })
	]);
	if (response.status === 404) error(404, 'Invoice not found.');
	if (!data) error(503, 'Could not load the invoice.');
	const approvalId = review.data?.canDecide ? review.data.approvalId : null;
	const approval = approvalId
		? (await client.GET('/api/v1/approvals/{id}', { params: { path: { id: approvalId } } })).data
		: null;
	const { user } = await event.parent();
	return {
		invoice: data,
		review: review.data ?? null,
		approval: approval ?? null,
		comments: comments.data ?? [],
		people: (users.data?.items ?? [])
			.filter((u) => u.status === 'active' && u.kind === 'human')
			.map((u) => ({ value: u.id, label: u.displayName })),
		me: user.id,
		canWrite: user.grantedPermissions.includes('billing:write'),
		canApprove: user.grantedPermissions.includes('billing:approve')
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });
const path = (id: string) => ({ params: { path: { id } } });

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let doc: components['schemas']['InvoiceInput'];
		try {
			doc = JSON.parse(formString(form, 'doc')) as components['schemas']['InvoiceInput'];
		} catch {
			return fail(400, { message: 'Could not read the invoice.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/invoices/{id}', {
			...path(event.params.id),
			body: doc
		});
		if (!data) return failed(response.status, problem, 'Could not save it.');
		return { message: 'Saved.' };
	},
	hold: async (event) => {
		const form = await event.request.formData();
		const on = formString(form, 'on') === 'yes';
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/hold', {
			...path(event.params.id),
			body: { onHold: on, reason: formString(form, 'reason') || null }
		});
		if (!data) return failed(response.status, problem, 'Could not change it.');
		return { message: on ? 'On hold.' : 'Released.' };
	},
	approve: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/approve', {
			...path(event.params.id),
			body: {
				version: Number(formString(form, 'version')),
				overrideReview: formString(form, 'override') === 'yes'
			}
		});
		if (!data) return failed(response.status, problem, 'Could not approve it.');
		return { message: 'Approved.' };
	},
	submitReview: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/review', {
			...path(event.params.id),
			body: { version: Number(formString(form, 'version')) }
		});
		if (!data) return failed(response.status, problem, 'Could not submit it for review.');
		return { message: 'Submitted for review.' };
	},
	decideReview: decide,
	comment: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/comments', {
			...path(event.params.id),
			body: {
				body: formString(form, 'body'),
				lineId: formString(form, 'lineId') || null,
				parentId: formString(form, 'parentId') || null,
				mentions: form.getAll('mentions').map(String)
			}
		});
		if (!data) return failed(response.status, problem, 'Could not add the comment.');
		return { message: 'Comment added.' };
	},
	resolve: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).POST(
			'/api/v1/invoices/{id}/comments/{comment}/resolve',
			{
				params: { path: { id: event.params.id, comment: formString(form, 'comment') } },
				body: { resolved: formString(form, 'resolved') === 'yes' }
			}
		);
		if (!response.ok) return failed(response.status, problem, 'Could not change it.');
		return { message: 'Done.' };
	},
	sent: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/sent', path(event.params.id));
		if (!data) return failed(response.status, problem, 'Could not mark it sent.');
		return { message: 'Marked sent.' };
	},
	void: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/void', {
			...path(event.params.id),
			body: { version: Number(formString(form, 'version')) }
		});
		if (!data) return failed(response.status, problem, 'Could not void it.');
		return { message: 'Voided: what it billed can be billed again.' };
	},
	credit: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/invoices/{id}/credit', {
			...path(event.params.id),
			body: {
				lineIds: form.getAll('lineIds').map(String),
				reason: formString(form, 'reason') || null
			}
		});
		if (!data) return failed(response.status, problem, 'Could not make the credit memo.');
		redirect(303, `/billing/invoices/${data.id}`);
	}
};
