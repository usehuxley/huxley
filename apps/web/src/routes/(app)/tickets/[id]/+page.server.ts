import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { actionsFor } from '#lib/forms/actions.server.ts';
import { parseDuration } from '#lib/tickets/duration.ts';
import { assignees, fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const path = { params: { path: { id: event.params.id } } };
	const [ticket, notes, activity, types, queues] = await Promise.all([
		client.GET('/api/v1/tickets/{id}', path),
		client.GET('/api/v1/tickets/{id}/notes', path),
		client.GET('/api/v1/tickets/{id}/activity', path),
		client.GET('/api/v1/servicedesk/types'),
		client.GET('/api/v1/queues')
	]);
	if (ticket.response.status === 404) error(404, 'Ticket not found.');
	if (!ticket.data || !notes.data || !activity.data || !types.data) {
		error(503, 'Could not load the ticket.');
	}
	const [people, contacts, responses, links, time, workTypes, actions] = await Promise.all([
		assignees(event),
		client.GET('/api/v1/orgs/{id}/contacts', {
			params: { path: { id: ticket.data.orgId }, query: { limit: 200 } }
		}),
		// Filled in for this ticket and you; none without ticket:write here.
		client.GET('/api/v1/tickets/{id}/responses', path),
		client.GET('/api/v1/tickets/{id}/links', path),
		client.GET('/api/v1/tickets/{id}/time', path),
		client.GET('/api/v1/work-types'),
		actionsFor(event, 'ticket', ticket.data.orgId)
	]);
	return {
		ticket: ticket.data,
		notes: notes.data,
		activity: activity.data,
		type: types.data.find((t) => t.key === ticket.data.typeKey) ?? null,
		queues: queues.data ?? [],
		...people,
		contacts: contacts.data?.items ?? [],
		responses: responses.data ?? [],
		links: links.data ?? [],
		time: time.data ?? [],
		workTypes: (workTypes.data ?? []).filter((w) => w.onTickets),
		actions
	};
};

/** A PATCH of the ticket; `body` holds only the fields to change. */
async function patch(
	event: Parameters<Actions[string]>[0],
	form: FormData,
	body: Record<string, unknown>,
	what: string
) {
	const { error: problem, response } = await api(event).PATCH('/api/v1/tickets/{id}', {
		params: { path: { id: event.params.id } },
		body: { version: Number(formString(form, 'version')), ...body }
	});
	if (response.status !== 200) {
		return fail(response.status, {
			[what]: {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not save the ticket.')
			}
		});
	}
	return { [what]: { saved: true } };
}

export const actions: Actions = {
	logTime: async (event) => {
		const form = await event.request.formData();
		const minutes = parseDuration(formString(form, 'duration'));
		if (minutes === null || Number.isNaN(minutes)) {
			return fail(400, {
				time: { errors: { minutes: 'A duration, like 30m or 1h 15m.' }, message: 'Not a duration.' }
			});
		}
		const { error: problem, response } = await api(event).POST('/api/v1/tickets/{id}/time', {
			params: { path: { id: event.params.id } },
			body: {
				workTypeId: formString(form, 'workTypeId'),
				minutes,
				workedOn: formString(form, 'workedOn') || null,
				billable: form.get('billable') === 'on',
				notes: formString(form, 'notes') || null
			}
		});
		if (response.status !== 201) {
			return fail(response.status, {
				time: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not log the time.')
				}
			});
		}
		return { time: { saved: true } };
	},
	deleteTime: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/time-entries/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				time: { errors: {}, message: problemMessage(problem, 'Could not delete it.') }
			});
		}
		return { time: { saved: true } };
	},
	link: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/tickets/{id}/links', {
			params: { path: { id: event.params.id } },
			body: {
				reference: formString(form, 'reference'),
				kind: formString(form, 'kind') || 'related'
			}
		});
		if (response.status !== 200) {
			return fail(response.status, {
				link: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not link the ticket.')
				}
			});
		}
		return { link: { saved: true } };
	},
	unlink: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/tickets/{id}/links/{other_id}',
			{ params: { path: { id: event.params.id, other_id: formString(form, 'otherId') } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				link: { errors: {}, message: problemMessage(problem, 'Could not unlink it.') }
			});
		}
		return { link: { saved: true } };
	},
	merge: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/tickets/{id}/merge', {
			params: { path: { id: event.params.id } },
			body: { into: formString(form, 'into') }
		});
		if (!data) {
			return fail(response.status, {
				merge: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not merge the ticket.')
				}
			});
		}
		redirect(303, `/tickets/${data.id}`);
	},
	properties: async (event) => {
		const form = await event.request.formData();
		const assignee = formString(form, 'assignee');
		return patch(
			event,
			form,
			{
				impact: Number(formString(form, 'impact')),
				urgency: Number(formString(form, 'urgency')),
				priority: formString(form, 'priority') || 'auto',
				queueId: formString(form, 'queueId') || null,
				requesterId: formString(form, 'requesterId') || null,
				assigneeUserId: assignee.startsWith('user:') ? assignee.slice(5) : null,
				assigneeTeamId: assignee.startsWith('team:') ? assignee.slice(5) : null
			},
			'properties'
		);
	},
	edit: async (event) => {
		const form = await event.request.formData();
		return patch(
			event,
			form,
			{ title: formString(form, 'title'), description: formString(form, 'description') },
			'edit'
		);
	},
	assign: async (event) => {
		const form = await event.request.formData();
		return patch(event, form, { assigneeUserId: formString(form, 'userId') }, 'properties');
	},
	status: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/tickets/{id}/status', {
			params: { path: { id: event.params.id } },
			body: {
				status: formString(form, 'status'),
				resolution: formString(form, 'resolution') || undefined,
				version: Number(formString(form, 'version'))
			}
		});
		if (response.status !== 200) {
			return fail(response.status, {
				status: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not change the status.')
				}
			});
		}
		return { status: { saved: true } };
	},
	note: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/tickets/{id}/notes', {
			params: { path: { id: event.params.id } },
			body: { kind: formString(form, 'kind'), body: formString(form, 'body') }
		});
		if (response.status !== 201) {
			return fail(response.status, {
				note: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not add the note.')
				}
			});
		}
		return { note: { saved: true } };
	}
};
