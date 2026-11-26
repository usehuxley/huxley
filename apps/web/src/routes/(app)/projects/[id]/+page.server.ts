import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [project, people] = await Promise.all([
		client.GET('/api/v1/projects/{id}', { params: { path: { id: event.params.id } } }),
		assignees(event)
	]);
	if (!project.data) error(project.response.status === 403 ? 403 : 404, 'There’s no such project.');
	const { user } = await event.parent();
	return {
		project: project.data,
		people: people.people,
		canWrite: user.grantedPermissions.includes('project:write'),
		canManage: user.grantedPermissions.includes('project:manage')
	};
};

export const actions: Actions = {
	milestone: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).POST('/api/v1/projects/{id}/milestones/{milestone}', {
			params: { path: { id: event.params.id, milestone: formString(form, 'milestone') } },
			body: { reached: formString(form, 'reached') === 'true' }
		});
		if (response.status !== 200)
			return fail(response.status, { message: 'Could not change the milestone.' });
		return { saved: true };
	},
	task: async (event) => {
		const form = await event.request.formData();
		const estimate = formString(form, 'estimateHours');
		const { error: problem, response } = await api(event).POST('/api/v1/projects/{id}/tasks', {
			params: { path: { id: event.params.id } },
			body: {
				title: formString(form, 'title'),
				phaseId: formString(form, 'phaseId') || null,
				assigneeUserId: formString(form, 'assigneeUserId') || null,
				estimateHours: estimate ? Number(estimate) : null,
				clientVisible: form.get('clientVisible') === 'on'
			}
		});
		if (response.status !== 201)
			return fail(response.status, { message: problemMessage(problem, 'Could not add the task.') });
		return { saved: true };
	},
	status: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PATCH('/api/v1/projects/{id}', {
			params: { path: { id: event.params.id } },
			body: { status: formString(form, 'status'), version: Number(formString(form, 'version')) }
		});
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not change it.') });
		return { saved: true };
	}
};
