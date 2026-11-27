import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/v1/invoices/{id}', {
		params: { path: { id: event.params.id } }
	});
	if (response.status === 404) error(404, 'Invoice not found.');
	if (!data) error(503, 'Could not load the invoice.');
	const { user } = await event.parent();
	return {
		invoice: data,
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
			body: { version: Number(formString(form, 'version')) }
		});
		if (!data) return failed(response.status, problem, 'Could not approve it.');
		return { message: 'Approved.' };
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
