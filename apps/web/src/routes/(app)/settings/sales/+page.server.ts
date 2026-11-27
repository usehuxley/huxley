import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

type Kind = 'open' | 'won' | 'lost';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [stages, reasons] = await Promise.all([
		client.GET('/api/v1/sales/stages'),
		client.GET('/api/v1/sales/reasons')
	]);
	if (!stages.data) error(stages.response.status === 404 ? 404 : 503, 'Could not load stages.');
	return { stages: stages.data, reasons: reasons.data ?? [] };
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	saveStage: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const body = {
			name: formString(form, 'name'),
			kind: (formString(form, 'kind') || 'open') as Kind,
			probability: Number(formString(form, 'probability') || '0')
		};
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = id
			? await client.PUT('/api/v1/sales/stages/{id}', { params: { path: { id } }, body })
			: await client.POST('/api/v1/sales/stages', { body });
		if (!data) return failed(response.status, problem, 'Could not save the stage.');
		return { saved: data.name };
	},
	archiveStage: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).DELETE('/api/v1/sales/stages/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (!response.ok) return failed(response.status, problem, 'Could not remove the stage.');
		return { archived: true };
	},
	reorder: async (event) => {
		const form = await event.request.formData();
		const ids = form.getAll('ids').map(String);
		const id = formString(form, 'id');
		const dir = formString(form, 'dir') === 'up' ? -1 : 1;
		const at = ids.indexOf(id);
		const to = at + dir;
		if (at < 0 || to < 0 || to >= ids.length) return { moved: false };
		[ids[at], ids[to]] = [ids[to], ids[at]];
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/sales/stages/order', {
			body: { ids }
		});
		if (!data) return failed(response.status, problem, 'Could not reorder the stages.');
		return { moved: true };
	},
	addReason: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/sales/reasons', {
			body: {
				kind: formString(form, 'kind') === 'lost' ? 'lost' : 'won',
				name: formString(form, 'name')
			}
		});
		if (!data) return failed(response.status, problem, 'Could not add the reason.');
		return { added: data.name };
	},
	archiveReason: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).DELETE('/api/v1/sales/reasons/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (!response.ok) return failed(response.status, problem, 'Could not remove the reason.');
		return { archived: true };
	}
};
