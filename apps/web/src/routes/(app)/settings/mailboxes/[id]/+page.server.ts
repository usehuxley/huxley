import { error, fail, redirect } from '@sveltejs/kit';
import { mailboxBody } from '#lib/mail/form.ts';
import { formOptions } from '#lib/mail/options.server.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const path = { params: { path: { id: event.params.id } } };
	const [mailbox, emails, options] = await Promise.all([
		client.GET('/api/v1/mailboxes/{id}', path),
		client.GET('/api/v1/mailboxes/{id}/emails', {
			params: { path: { id: event.params.id }, query: { limit: 50 } }
		}),
		formOptions(event)
	]);
	if (mailbox.response.status === 404) error(404, 'Mailbox not found.');
	if (!mailbox.data) error(503, 'Could not load the mailbox.');
	return { mailbox: mailbox.data, emails: emails.data ?? [], ...options };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT('/api/v1/mailboxes/{id}', {
			params: { path: { id: event.params.id } },
			body: mailboxBody(form)
		});
		if (response.status !== 200) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the mailbox.')
			});
		}
		return { saved: true };
	},
	archive: async (event) => {
		const { error: problem, response } = await api(event).DELETE('/api/v1/mailboxes/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				errors: {},
				message: problemMessage(problem, 'Could not archive the mailbox.')
			});
		}
		redirect(303, '/settings/mailboxes');
	}
};
