import { fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [orgs, templates, people] = await Promise.all([
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/project-templates'),
		assignees(event)
	]);
	return {
		companies: (orgs.data ?? [])
			.filter((o) => o.kind === 'client')
			.map((o) => ({ value: o.id, label: o.name })),
		templates: templates.data ?? [],
		people: people.people
	};
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const assigneesByKey: Record<string, string> = {};
		for (const [key, value] of form.entries()) {
			if (key.startsWith('assign.') && typeof value === 'string' && value)
				assigneesByKey[key.slice(7)] = value;
		}
		const budget = formString(form, 'budgetHours');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/projects', {
			body: {
				orgId: formString(form, 'orgId'),
				name: formString(form, 'name'),
				description: formString(form, 'description'),
				startOn: formString(form, 'startOn'),
				endOn: formString(form, 'endOn') || null,
				templateId: formString(form, 'templateId') || null,
				budgetHours: budget ? Number(budget) : null,
				billingMethod: formString(form, 'billingMethod'),
				assignees: assigneesByKey
			}
		});
		if (!data)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the project.')
			});
		redirect(303, `/projects/${data.id}`);
	}
};
