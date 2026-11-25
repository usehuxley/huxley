import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { VIEWS } from '#lib/tickets/labels.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees, fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

const PAGE = 50;

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const view = (VIEWS.find((v) => v.value === params.get('view')) ?? VIEWS[0]).value;
	const queueId = params.get('queue') || undefined;
	const orgId = params.get('org') || undefined;
	const priority = params.get('priority') || undefined;
	const search = params.get('q') ?? '';
	const page = Math.max(1, Number(params.get('page')) || 1);
	const client = api(event);
	const canWrite = (await event.parent()).user.grantedPermissions.includes('ticket:write');
	const [tickets, queues, views, people, types] = await Promise.all([
		client.GET('/api/v1/tickets', {
			params: {
				query: {
					...VIEWS.find((v) => v.value === view)!.query,
					queueId,
					orgId,
					priority,
					search: search || undefined,
					limit: PAGE,
					offset: (page - 1) * PAGE
				}
			}
		}),
		client.GET('/api/v1/queues'),
		client.GET('/api/v1/ticket-views'),
		canWrite ? assignees(event) : { people: [], teams: [] },
		canWrite ? client.GET('/api/v1/servicedesk/types') : null
	]);
	// Statuses every type has, for changing many tickets at once.
	const typeList = types?.data ?? [];
	const statuses = (typeList[0]?.statuses ?? [])
		.filter((st) => typeList.every((t) => t.statuses.some((o) => o.key === st.key)))
		.map((st) => ({ value: st.key, label: st.name }));
	if (tickets.response.status === 404) error(404, 'The service desk is turned off.');
	if (!tickets.data) error(503, 'Could not load tickets.');
	return {
		tickets: tickets.data.items,
		total: tickets.data.total,
		queues: queues.data ?? [],
		views: views.data ?? [],
		...people,
		statuses,
		saved: params.get('saved') ?? '',
		view,
		queueId: queueId ?? '',
		orgId: orgId ?? '',
		priority: priority ?? '',
		search,
		page,
		pages: Math.max(1, Math.ceil(tickets.data.total / PAGE))
	};
};

export const actions: Actions = {
	bulk: async (event) => {
		const form = await event.request.formData();
		const ids = form.getAll('ids').map(String);
		const assignee = formString(form, 'assignee');
		const priority = formString(form, 'priority');
		const queue = formString(form, 'queue');
		const changes = {
			...(assignee === 'none'
				? { assigneeUserId: null, assigneeTeamId: null }
				: assignee.startsWith('user:')
					? { assigneeUserId: assignee.slice(5) }
					: assignee.startsWith('team:')
						? { assigneeTeamId: assignee.slice(5), assigneeUserId: null }
						: {}),
			...(priority ? { priority } : {}),
			...(queue === 'none' ? { queueId: null } : queue ? { queueId: queue } : {})
		};
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/tickets/bulk', {
			body: { ids, changes, status: formString(form, 'status') || null }
		});
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not change the tickets.')
			});
		}
		const failed = data.filter((r) => !r.ok);
		return {
			bulk: {
				changed: data.length - failed.length,
				failed: failed.length,
				reason: failed[0]?.error ?? null
			}
		};
	},
	saveView: async (event) => {
		const form = await event.request.formData();
		const filters = {
			view: formString(form, 'view'),
			queueId: formString(form, 'queueId'),
			orgId: formString(form, 'orgId'),
			search: formString(form, 'search'),
			priority: formString(form, 'priority')
		};
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/ticket-views', {
			body: { name: formString(form, 'name'), filters, shared: form.get('shared') === 'on' }
		});
		if (!data) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the view.')
			});
		}
		const url = new URL(event.url);
		url.search = '';
		for (const [key, value] of Object.entries({
			view: filters.view === 'mine' ? '' : filters.view,
			queue: filters.queueId,
			org: filters.orgId,
			q: filters.search,
			priority: filters.priority,
			saved: data.id
		})) {
			if (value) url.searchParams.set(key, value);
		}
		redirect(303, `${url.pathname}${url.search}`);
	},
	deleteView: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/ticket-views/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				errors: {},
				message: problemMessage(problem, 'Could not delete the view.')
			});
		}
		redirect(303, '/tickets');
	}
};
