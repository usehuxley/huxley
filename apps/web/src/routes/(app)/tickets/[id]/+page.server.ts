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
	const { user } = await event.parent();
	const [
		people,
		contacts,
		responses,
		links,
		time,
		workTypes,
		actions,
		appointments,
		knowledge,
		assets,
		coverage,
		csat,
		sentiment,
		recommendations,
		insight,
		similar
	] = await Promise.all([
		assignees(event),
		client.GET('/api/v1/orgs/{id}/contacts', {
			params: { path: { id: ticket.data.orgId }, query: { limit: 200 } }
		}),
		// Filled in for this ticket and you; none without ticket:write here.
		client.GET('/api/v1/tickets/{id}/responses', path),
		client.GET('/api/v1/tickets/{id}/links', path),
		client.GET('/api/v1/tickets/{id}/time', path),
		client.GET('/api/v1/work-types'),
		actionsFor(event, 'ticket', ticket.data.orgId),
		client.GET('/api/v1/tickets/{id}/appointments', path),
		user.activeModules.includes('knowledge') && user.grantedPermissions.includes('kb:read')
			? client.GET('/api/v1/kb/tickets/{ticket_id}/articles', {
					params: { path: { ticket_id: event.params.id } }
				})
			: Promise.resolve({ data: undefined }),
		user.activeModules.includes('cmdb') && user.grantedPermissions.includes('asset:read')
			? client.GET('/api/v1/tickets/{ticket_id}/assets', {
					params: { path: { ticket_id: event.params.id } }
				})
			: Promise.resolve({ data: undefined }),
		user.activeModules.includes('contracts') && user.grantedPermissions.includes('agreement:read')
			? client.GET('/api/v1/tickets/{id}/time-coverage', path)
			: Promise.resolve({ data: undefined }),
		user.activeModules.includes('cx')
			? client.GET('/api/v1/tickets/{id}/csat', path)
			: Promise.resolve({ data: undefined }),
		user.activeModules.includes('cx')
			? client.GET('/api/v1/tickets/{id}/sentiment', path)
			: Promise.resolve({ data: undefined }),
		user.grantedPermissions.includes('ticket:write')
			? client.GET('/api/v1/tickets/{id}/recommendations', {
					params: { path: { id: event.params.id }, query: { limit: 3 } }
				})
			: Promise.resolve({ data: undefined }),
		client.GET('/api/v1/tickets/{id}/insight', path),
		client.GET('/api/v1/tickets/{id}/similar', {
			params: { path: { id: event.params.id }, query: { limit: 5 } }
		})
	]);
	// A change ticket: its change details, the risk questions, its impact
	// and the client's assets to pick from.
	const isChange = ticket.data.typeKey === 'change' && user.activeModules.includes('change');
	const [change, questions, impact, changeAssets] = isChange
		? await Promise.all([
				client.GET('/api/v1/tickets/{id}/change', path),
				client.GET('/api/v1/changes/questions'),
				client.GET('/api/v1/tickets/{id}/change/impact', path),
				user.activeModules.includes('cmdb') && user.grantedPermissions.includes('asset:read')
					? client.GET('/api/v1/assets', {
							params: { query: { orgId: ticket.data.orgId, inUse: true, limit: 200 } }
						})
					: Promise.resolve({ data: undefined })
			])
		: [{ data: undefined }, { data: undefined }, { data: undefined }, { data: undefined }];
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
		actions,
		appointments: appointments.data ?? [],
		knowledge: knowledge.data ?? null,
		assets: assets.data ?? null,
		coverage: coverage.data ?? null,
		csat: csat.data ?? null,
		sentiment: sentiment.data ?? [],
		recommendations: recommendations.data ?? [],
		insight: insight.data ?? null,
		change: change.data ?? null,
		changeQuestions: questions.data ?? [],
		changeImpact: impact.data ?? null,
		changeAssets:
			(changeAssets.data as { items?: { id: string; name: string }[] } | undefined)?.items ?? [],
		similar: similar.data ?? null
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
	linkAsset: async (event) => {
		const assetId = formString(await event.request.formData(), 'assetId');
		const { error: problem, response } = await api(event).POST(
			'/api/v1/tickets/{ticket_id}/assets',
			{
				params: { path: { ticket_id: event.params.id } },
				body: { assetId }
			}
		);
		if (response.status !== 204) {
			return fail(response.status, {
				assets: { message: problemMessage(problem, 'Could not link it.') }
			});
		}
		return { assets: { message: 'Asset linked.' } };
	},
	unlinkAsset: async (event) => {
		const assetId = formString(await event.request.formData(), 'assetId');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/tickets/{ticket_id}/assets/{asset_id}',
			{ params: { path: { ticket_id: event.params.id, asset_id: assetId } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				assets: { message: problemMessage(problem, 'Could not remove it.') }
			});
		}
		return { assets: { message: 'Asset removed.' } };
	},
	linkArticle: async (event) => {
		const articleId = formString(await event.request.formData(), 'articleId');
		const { error: problem, response } = await api(event).POST(
			'/api/v1/kb/tickets/{ticket_id}/articles',
			{
				params: { path: { ticket_id: event.params.id } },
				body: { articleId }
			}
		);
		if (response.status !== 204) {
			return fail(response.status, {
				knowledge: { message: problemMessage(problem, 'Could not link it.') }
			});
		}
		return { knowledge: { message: 'Article linked.' } };
	},
	unlinkArticle: async (event) => {
		const articleId = formString(await event.request.formData(), 'articleId');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/kb/tickets/{ticket_id}/articles/{article_id}',
			{ params: { path: { ticket_id: event.params.id, article_id: articleId } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				knowledge: { message: problemMessage(problem, 'Could not unlink it.') }
			});
		}
		return { knowledge: { message: 'Article unlinked.' } };
	},
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
	book: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/tickets/{id}/appointments',
			{
				params: { path: { id: event.params.id } },
				body: {
					userId: formString(form, 'userId'),
					startsAt: formString(form, 'startsAt'),
					endsAt: formString(form, 'endsAt'),
					notes: formString(form, 'notes') || null
				}
			}
		);
		if (response.status !== 201) {
			return fail(response.status, {
				appointment: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not book it.')
				}
			});
		}
		return { appointment: { saved: true } };
	},
	cancelAppointment: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/appointments/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				appointment: { errors: {}, message: problemMessage(problem, 'Could not cancel it.') }
			});
		}
		return { appointment: { saved: true } };
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
	saveChange: async (event) => {
		const form = await event.request.formData();
		const answers: Record<string, string> = {};
		for (const [key, value] of form.entries()) {
			if (key.startsWith('risk.') && typeof value === 'string' && value)
				answers[key.slice(5)] = value;
		}
		const iso = (v: string) => (v ? new Date(v).toISOString() : null);
		const { error: problem, response } = await api(event).PUT('/api/v1/tickets/{id}/change', {
			params: { path: { id: event.params.id } },
			body: {
				changeType: formString(form, 'changeType'),
				riskAnswers: answers,
				implementationPlan: formString(form, 'implementationPlan'),
				backoutPlan: formString(form, 'backoutPlan'),
				testPlan: formString(form, 'testPlan'),
				plannedStart: iso(formString(form, 'plannedStart')),
				plannedEnd: iso(formString(form, 'plannedEnd')),
				affectedAssets: form.getAll('asset').map(String),
				version: Number(formString(form, 'version'))
			}
		});
		if (response.status !== 200) {
			return fail(response.status, {
				change: { message: problemMessage(problem, 'Could not save the change.') }
			});
		}
		return { change: { saved: true } };
	},
	submitChange: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/tickets/{id}/change/submit', {
			params: { path: { id: event.params.id } }
		});
		if (!data) {
			return fail(response.status, {
				change: { message: problemMessage(problem, 'Could not submit it.') }
			});
		}
		return {
			change: {
				submitted: true,
				conflicts: data.conflicts.map((c) => `${c.reference} · ${c.title}`)
			}
		};
	},
	implementedChange: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/tickets/{id}/change/implemented',
			{
				params: { path: { id: event.params.id } },
				body: { outcome: formString(form, 'outcome') }
			}
		);
		if (response.status !== 200) {
			return fail(response.status, {
				change: { message: problemMessage(problem, 'Could not record it.') }
			});
		}
		return { change: { saved: true } };
	},
	reviewChange: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/tickets/{id}/change/review',
			{
				params: { path: { id: event.params.id } },
				body: { review: formString(form, 'review') }
			}
		);
		if (response.status !== 200) {
			return fail(response.status, {
				change: { message: problemMessage(problem, 'Could not save the review.') }
			});
		}
		return { change: { saved: true } };
	},
	insight: async (event) => {
		const { response } = await api(event).POST('/api/v1/tickets/{id}/insight/refresh', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 202)
			return fail(response.status, { insight: { message: 'Could not ask for it.' } });
		return { insight: { asked: true } };
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
