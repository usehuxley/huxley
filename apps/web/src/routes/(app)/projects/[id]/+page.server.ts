import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

const VIEWS = ['list', 'board', 'timeline'] as const;
const isView = (v: string | null): v is (typeof VIEWS)[number] =>
	(VIEWS as readonly string[]).includes(v ?? '');

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [project, people] = await Promise.all([
		client.GET('/api/v1/projects/{id}', { params: { path: { id: event.params.id } } }),
		assignees(event)
	]);
	if (!project.data) error(project.response.status === 403 ? 403 : 404, 'There’s no such project.');
	const { user } = await event.parent();
	const canManage = user.grantedPermissions.includes('project:manage');
	const agreements = canManage
		? (
				(
					await client.GET('/api/v1/agreements', {
						params: { query: { orgId: project.data.orgId } }
					})
				).data ?? []
			)
				.filter((a) => a.orgId === project.data.orgId && a.status !== 'cancelled')
				.map((a) => ({ value: a.id, label: a.name }))
		: [];
	const asked = event.url.searchParams.get('view');
	return {
		project: project.data,
		people: people.people,
		agreements,
		view: isView(asked) ? asked : 'list',
		canWrite: user.grantedPermissions.includes('project:write'),
		canManage
	};
};

const num = (form: FormData, key: string) => {
	const v = formString(form, key);
	return v ? Number(v) : null;
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
		const { error: problem, response } = await api(event).POST('/api/v1/projects/{id}/tasks', {
			params: { path: { id: event.params.id } },
			body: {
				title: formString(form, 'title'),
				phaseId: formString(form, 'phaseId') || null,
				assigneeUserId: formString(form, 'assigneeUserId') || null,
				estimateHours: num(form, 'estimateHours'),
				startOn: formString(form, 'startOn') || null,
				dueOn: formString(form, 'dueOn') || null,
				clientVisible: form.get('clientVisible') === 'on'
			}
		});
		if (response.status !== 201)
			return fail(response.status, { message: problemMessage(problem, 'Could not add the task.') });
		return { saved: true };
	},
	editTask: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PATCH(
			'/api/v1/projects/{id}/tasks/{ticket}',
			{
				params: { path: { id: event.params.id, ticket: formString(form, 'ticket') } },
				body: {
					phaseId: formString(form, 'phaseId') || null,
					estimateHours: num(form, 'estimateHours'),
					startOn: formString(form, 'startOn'),
					dueOn: formString(form, 'dueOn'),
					clientVisible: form.get('clientVisible') === 'on'
				}
			}
		);
		if (response.status !== 200)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not change the task.')
			});
		return { saved: true };
	},
	status: async (event) => {
		const form = await event.request.formData();
		const agreement = formString(form, 'agreementId');
		const { error: problem, response } = await api(event).PATCH('/api/v1/projects/{id}', {
			params: { path: { id: event.params.id } },
			body: {
				status: formString(form, 'status'),
				agreementId: agreement || null,
				clearAgreement: !agreement,
				version: Number(formString(form, 'version'))
			}
		});
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not change it.') });
		return { saved: true };
	},
	scope: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/projects/{id}/scope-changes',
			{
				params: { path: { id: event.params.id } },
				body: {
					title: formString(form, 'title'),
					description: formString(form, 'description'),
					hours: num(form, 'hours') ?? 0,
					amount: num(form, 'amount')
				}
			}
		);
		if (response.status !== 201)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not add the scope change.')
			});
		return { saved: true };
	},
	submitScope: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/projects/{id}/scope-changes/{change}/submit',
			{
				params: { path: { id: event.params.id, change: formString(form, 'change') } }
			}
		);
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not submit it.') });
		return { saved: true };
	},
	withdrawScope: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/projects/{id}/scope-changes/{change}/withdraw',
			{
				params: { path: { id: event.params.id, change: formString(form, 'change') } }
			}
		);
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not withdraw it.') });
		return { saved: true };
	}
};
