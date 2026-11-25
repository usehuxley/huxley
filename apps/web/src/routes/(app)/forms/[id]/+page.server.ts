import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [found, orgs] = await Promise.all([
		client.GET('/api/v1/forms/{id}', { params: { path: { id: event.params.id } } }),
		client.GET('/api/v1/orgs')
	]);
	if (found.response.status === 404) error(404, 'Form not found.');
	if (!found.data) error(503, 'Could not load the form.');
	// Run as an Action from a record: what it's on, for the page to say.
	const params = event.url.searchParams;
	const contactId = params.get('contact') ?? '';
	const ticketId = params.get('ticket') ?? '';
	const [contact, ticket] = await Promise.all([
		contactId ? client.GET('/api/v1/contacts/{id}', { params: { path: { id: contactId } } }) : null,
		ticketId ? client.GET('/api/v1/tickets/{id}', { params: { path: { id: ticketId } } }) : null
	]);
	const orgId = params.get('org') ?? '';
	const on = orgId
		? {
				orgId,
				orgName: (orgs.data ?? []).find((o) => o.id === orgId)?.name ?? 'the client',
				contactId: contact?.data?.id ?? '',
				contactName: contact?.data?.name ?? '',
				ticketId: ticket?.data?.id ?? '',
				ticketReference: ticket?.data?.reference ?? '',
				ticketTitle: ticket?.data?.title ?? ''
			}
		: null;
	return { form: found.data, orgs: orgs.data ?? [], on };
};

/** Field errors by name, from the API's `answers/<field>` pointers. */
function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	const list = (problem as { errors?: { pointer?: string; message?: string }[] } | undefined)
		?.errors;
	for (const e of list ?? []) {
		const name = e.pointer?.replace(/^answers\/?/, '').split('/')[0];
		if (name && e.message) errors[name] = e.message;
	}
	return errors;
}

export const actions: Actions = {
	submit: async (event) => {
		const form = await event.request.formData();
		let answers: Record<string, unknown>;
		try {
			answers = JSON.parse(formString(form, 'answers') || '{}') as Record<string, unknown>;
		} catch {
			return fail(400, { message: 'Could not read the answers.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/forms/{id}/submissions', {
			params: { path: { id: event.params.id } },
			body: {
				orgId: formString(form, 'orgId') || null,
				answers,
				context:
					formString(form, 'contactId') || formString(form, 'ticketId')
						? {
								contactId: formString(form, 'contactId') || null,
								ticketId: formString(form, 'ticketId') || null
							}
						: null
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not submit it.'),
				errors: fieldErrors(problem)
			});
		}
		return { submitted: data };
	}
};
