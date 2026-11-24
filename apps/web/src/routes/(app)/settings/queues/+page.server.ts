import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [queues, orgs, people] = await Promise.all([
		client.GET('/api/v1/queues'),
		client.GET('/api/v1/orgs'),
		assignees(event)
	]);
	if (queues.response.status === 404) error(404, 'The service desk is turned off.');
	if (!queues.data) error(503, 'Could not load queues.');
	// What each queue requires (with the workforce module, for those who keep it).
	const { user } = await event.parent();
	const workforce =
		user.activeModules.includes('workforce') &&
		user.grantedPermissions.includes('workforce:manage');
	const [skills, certifications, ...required] = workforce
		? await Promise.all([
				client.GET('/api/v1/workforce/skills'),
				client.GET('/api/v1/workforce/certifications'),
				...queues.data.map((q) =>
					client.GET('/api/v1/workforce/requirements/{target_type}/{target_id}', {
						params: { path: { target_type: 'queue', target_id: q.id } }
					})
				)
			])
		: [];
	return {
		workforce,
		skills: skills?.data ?? [],
		certifications: certifications?.data ?? [],
		requirements: Object.fromEntries(queues.data.map((q, i) => [q.id, required[i]?.data ?? []])),
		queues: queues.data,
		// Queues usually belong to your MSP: provider organizations.
		providers: (orgs.data ?? [])
			.filter((o) => o.kind === 'provider' && o.status === 'active')
			.map((o) => ({ value: o.id, label: o.name })),
		...people
	};
};

/** Name, description and members from the queue form. */
function queueBody(form: FormData) {
	return {
		name: formString(form, 'name'),
		description: formString(form, 'description'),
		userIds: form.getAll('userIds').map(String),
		teamIds: form.getAll('teamIds').map(String)
	};
}

export const actions: Actions = {
	requirements: async (event) => {
		const form = await event.request.formData();
		const skills = form.getAll('skillId').map(String);
		const levels = form.getAll('minLevel').map(String);
		const body = [
			...skills
				.map((skillId, i) => ({ skillId, minLevel: Number(levels[i]) }))
				.filter((r) => r.skillId),
			...form
				.getAll('certificationId')
				.map(String)
				.map((certificationId) => ({ certificationId }))
		];
		const { error: problem, response } = await api(event).PUT(
			'/api/v1/workforce/requirements/{target_type}/{target_id}',
			{ params: { path: { target_type: 'queue', target_id: formString(form, 'id') } }, body }
		);
		if (response.status !== 200) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the requirements.')
			});
		}
		return { requirements: true };
	},
	create: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/queues', {
			body: { orgId: formString(form, 'orgId'), ...queueBody(form) }
		});
		if (response.status !== 201) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the queue.')
			});
		}
		return { created: true };
	},
	update: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT('/api/v1/queues/{id}', {
			params: { path: { id: formString(form, 'id') } },
			body: { ...queueBody(form), version: Number(formString(form, 'version')) }
		});
		if (response.status !== 200) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the queue.')
			});
		}
		return { saved: true };
	},
	archive: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE('/api/v1/queues/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not archive the queue.')
			});
		}
		return { archived: true };
	}
};
