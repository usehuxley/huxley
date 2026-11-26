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
	const canWrite = user.grantedPermissions.includes('project:write');
	const suggestions = canWrite ? await suggest(event, project.data) : {};
	const asked = event.url.searchParams.get('view');
	return {
		project: project.data,
		people: people.people,
		agreements,
		view: isView(asked) ? asked : 'list',
		suggestions,
		canWrite,
		canManage
	};
};

type Suggestion = {
	userId: string;
	name: string;
	skillMatch: number;
	missing: string[];
	freeHours: number | null;
};

/** Who could take each unassigned open task: qualified, available and
 * skilled first, with their free hours this week. */
async function suggest(
	event: Parameters<PageServerLoad>[0],
	project: { tasks: { ticketId: string; assigneeUserId?: string | null; statusCategory: string }[] }
): Promise<Record<string, Suggestion[]>> {
	const open = project.tasks
		.filter(
			(t) => !t.assigneeUserId && t.statusCategory !== 'resolved' && t.statusCategory !== 'closed'
		)
		.slice(0, 10);
	if (open.length === 0) return {};
	const client = api(event);
	const capacity = await client.GET('/api/v1/projects/capacity', {
		params: { query: { weeks: 1 } }
	});
	const free = new Map(
		(capacity.data?.people ?? []).map((p) => [
			p.userId,
			Math.max(0, (p.weeks[0]?.capacityHours ?? 0) - (p.weeks[0]?.bookedHours ?? 0))
		])
	);
	const fits = await Promise.all(
		open.map((t) =>
			client.GET('/api/v1/workforce/fit', { params: { query: { ticketId: t.ticketId } } })
		)
	);
	const out: Record<string, Suggestion[]> = {};
	open.forEach((t, i) => {
		const people = (fits[i].data ?? []).filter((f) => f.qualified && f.available);
		if (people.length === 0) return;
		out[t.ticketId] = people
			.map((f) => ({
				userId: f.userId,
				name: f.name,
				skillMatch: Math.round(f.skillMatch * 100),
				missing: f.missing,
				freeHours: free.has(f.userId) ? Math.round(free.get(f.userId)! * 10) / 10 : null
			}))
			.sort((a, b) => b.skillMatch - a.skillMatch || (b.freeHours ?? 0) - (a.freeHours ?? 0))
			.slice(0, 3);
	});
	return out;
}

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
	assign: async (event) => {
		const form = await event.request.formData();
		const client = api(event);
		const ticket = formString(form, 'ticket');
		const current = await client.GET('/api/v1/tickets/{id}', { params: { path: { id: ticket } } });
		if (!current.data) return fail(404, { message: 'There’s no such task.' });
		const { error: problem, response } = await client.PATCH('/api/v1/tickets/{id}', {
			params: { path: { id: ticket } },
			body: { version: current.data.version, assigneeUserId: formString(form, 'userId') }
		});
		if (response.status !== 200)
			return fail(response.status, { message: problemMessage(problem, 'Could not assign it.') });
		return { saved: true };
	},
	report: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/projects/{id}/status-reports',
			{
				params: { path: { id: event.params.id } },
				body: { body: formString(form, 'body'), clientVisible: form.get('clientVisible') === 'on' }
			}
		);
		if (response.status !== 201)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the report.')
			});
		return { saved: true };
	},
	shareReport: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).PATCH('/api/v1/projects/{id}/status-reports/{report}', {
			params: { path: { id: event.params.id, report: formString(form, 'report') } },
			body: { clientVisible: formString(form, 'visible') === 'true' }
		});
		if (response.status !== 200)
			return fail(response.status, { message: 'Could not change sharing.' });
		return { saved: true };
	},
	deleteReport: async (event) => {
		const form = await event.request.formData();
		await api(event).DELETE('/api/v1/projects/{id}/status-reports/{report}', {
			params: { path: { id: event.params.id, report: formString(form, 'report') } }
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
