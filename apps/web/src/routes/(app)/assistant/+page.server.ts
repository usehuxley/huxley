import { fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { currentOrg } = await event.parent();
	const client = api(event);
	const [list, orgs] = await Promise.all([
		client.GET('/api/v1/assistant/conversations'),
		client.GET('/api/v1/orgs')
	]);
	const id = event.url.searchParams.get('c');
	// Opened from a ticket or a client: what a new conversation is about.
	const ticketId = event.url.searchParams.get('ticket');
	const clientId = event.url.searchParams.get('client');
	let context: { kind: 'ticket' | 'client'; id: string; label: string } | null = null;
	if (ticketId) {
		const { data } = await client.GET('/api/v1/tickets/{id}', {
			params: { path: { id: ticketId } }
		});
		if (data) context = { kind: 'ticket', id: data.id, label: `${data.reference} · ${data.title}` };
	} else if (clientId) {
		const { data } = await client.GET('/api/v1/orgs/{id}', { params: { path: { id: clientId } } });
		if (data) context = { kind: 'client', id: data.id, label: data.name };
	}
	const conversation = id
		? ((await client.GET('/api/v1/assistant/conversations/{id}', { params: { path: { id } } }))
				.data ?? null)
		: null;
	return {
		conversations: list.data ?? [],
		conversation,
		context,
		orgId: currentOrg?.id ?? orgs.data?.[0]?.id ?? ''
	};
};

export const actions: Actions = {
	start: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/assistant/conversations', {
			body: {
				orgId: formString(form, 'orgId'),
				text: formString(form, 'text'),
				...(formString(form, 'contextId')
					? {
							context: { kind: formString(form, 'contextKind'), id: formString(form, 'contextId') }
						}
					: {})
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'The assistant could not answer.')
			});
		}
		redirect(303, `/assistant?c=${data.id}`);
	},
	say: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/assistant/conversations/{id}/messages', {
			params: { path: { id } },
			body: { text: formString(form, 'text') }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'The assistant could not answer.')
			});
		}
		return { sent: true };
	},
	decide: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const decisions: Record<string, boolean> = {};
		for (const [key, value] of form.entries()) {
			if (key.startsWith('call:')) decisions[key.slice(5)] = value === 'approve';
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/assistant/conversations/{id}/decisions', {
			params: { path: { id } },
			body: { decisions }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'That did not work.') });
		}
		return { sent: true };
	},
	delete: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		await api(event).DELETE('/api/v1/assistant/conversations/{id}', { params: { path: { id } } });
		redirect(303, '/assistant');
	}
};
