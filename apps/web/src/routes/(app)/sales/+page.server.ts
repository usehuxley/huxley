import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

/** The pipeline: open opportunities by stage, or a list of any status. */
export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const view = params.get('view') === 'list' ? 'list' : 'board';
	const status = (['open', 'won', 'lost'] as const).find((s) => s === params.get('status'));
	const mine = params.get('mine') === '1';
	const client = api(event);
	const { user } = await event.parent();
	const [stages, reasons, opportunities] = await Promise.all([
		client.GET('/api/v1/sales/stages'),
		client.GET('/api/v1/sales/reasons'),
		client.GET('/api/v1/opportunities', {
			params: {
				query: {
					status: view === 'board' ? 'open' : status,
					ownerId: mine ? user.id : undefined,
					q: params.get('q') || undefined
				}
			}
		})
	]);
	if (stages.response.status === 404) error(404, 'Sales is turned off.');
	if (!stages.data || !opportunities.data) error(503, 'Could not load the pipeline.');
	return {
		view,
		status: status ?? '',
		mine,
		q: params.get('q') ?? '',
		stages: stages.data,
		reasons: reasons.data ?? [],
		opportunities: opportunities.data,
		canWrite: user.grantedPermissions.includes('sales:write')
	};
};

export const actions: Actions = {
	move: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/opportunities/{id}/move', {
			params: { path: { id: formString(form, 'id') } },
			body: {
				stageId: formString(form, 'stageId'),
				reasonId: formString(form, 'reasonId') || null,
				reasonNote: formString(form, 'reasonNote') || null,
				version: Number(formString(form, 'version'))
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not move it.')
			});
		}
		return { moved: data.name };
	}
};
