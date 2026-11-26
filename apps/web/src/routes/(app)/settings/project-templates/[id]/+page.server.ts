import { error, fail, redirect } from '@sveltejs/kit';
import { homeOrg } from '#lib/dashboards/server.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	if (event.params.id === 'new') return { template: null, workflows: [] };
	const client = api(event);
	const [template, workflows] = await Promise.all([
		client.GET('/api/v1/project-templates/{id}', { params: { path: { id: event.params.id } } }),
		client.GET('/api/v1/project-templates/{id}/workflows', {
			params: { path: { id: event.params.id } }
		})
	]);
	if (!template.data) error(404, 'There’s no such template.');
	return { template: template.data, workflows: workflows.data ?? [] };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		let structure: unknown;
		try {
			structure = JSON.parse(formString(form, 'structure'));
		} catch {
			return fail(422, { message: 'The template couldn’t be read.' });
		}
		const body = {
			orgId: formString(form, 'orgId') || ((await homeOrg(event)) ?? ''),
			name: formString(form, 'name'),
			description: formString(form, 'description'),
			structure: structure as never,
			version: formString(form, 'version') ? Number(formString(form, 'version')) : null
		};
		const result =
			event.params.id === 'new'
				? await api(event).POST('/api/v1/project-templates', { body })
				: await api(event).PUT('/api/v1/project-templates/{id}', {
						params: { path: { id: event.params.id } },
						body
					});
		if (!result.data) {
			return fail(result.response.status, {
				message: problemMessage(result.error, 'Could not save the template.')
			});
		}
		redirect(303, '/settings/project-templates');
	},
	// A draft workflow on this template's new projects, to fill in.
	workflow: async (event) => {
		const client = api(event);
		const { data: template } = await client.GET('/api/v1/project-templates/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (!template) error(404, 'There’s no such template.');
		const slug =
			template.name
				.toLowerCase()
				.replace(/[^a-z0-9]+/g, '-')
				.replace(/^-|-$/g, '')
				.slice(0, 40) || 'project';
		const {
			data,
			error: problem,
			response
		} = await client.POST('/api/v1/workflows', {
			body: {
				orgId: template.orgId,
				document: {
					apiVersion: 'usehuxley.io/v1',
					kind: 'Workflow',
					metadata: {
						key: `${slug}-kickoff-${Date.now().toString(36)}`,
						name: `${template.name}: kickoff`
					},
					spec: {
						triggers: [
							{
								type: 'event',
								event: 'project.created',
								filter: `{{ event.payload.templateId == '${template.id}' }}`
							}
						],
						steps: [{ id: 'project', type: 'transform', output: '{{ inputs }}' }]
					}
				} as never
			}
		});
		if (!data)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the workflow.')
			});
		redirect(303, `/workflows/${data.id}`);
	},
	remove: async (event) => {
		await api(event).DELETE('/api/v1/project-templates/{id}', {
			params: { path: { id: event.params.id } }
		});
		redirect(303, '/settings/project-templates');
	}
};
