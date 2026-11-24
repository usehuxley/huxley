import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { mailboxBody } from '#lib/mail/form.ts';
import { formOptions } from '#lib/mail/options.server.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const [mailboxes, options] = await Promise.all([
		api(event).GET('/api/v1/mailboxes'),
		formOptions(event)
	]);
	if (mailboxes.response.status === 404) error(404, 'The service desk is turned off.');
	if (!mailboxes.data) error(503, 'Could not load mailboxes.');
	return { mailboxes: mailboxes.data, ...options };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/mailboxes', {
			body: { orgId: formString(form, 'orgId'), ...mailboxBody(form) }
		});
		if (response.status !== 201 || !data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not add the mailbox.')
			});
		}
		redirect(303, `/settings/mailboxes/${data.id}`);
	}
};
