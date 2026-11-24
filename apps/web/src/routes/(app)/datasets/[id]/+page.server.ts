import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

const PAGE = 50;

export const load: PageServerLoad = async (event) => {
	const id = event.params.id;
	const client = api(event);
	const { data: dataset, response } = await client.GET('/api/v1/datasets/{id}', {
		params: { path: { id } }
	});
	if (response.status === 404) error(404, 'No such dataset.');
	if (!dataset) error(503, 'Could not load the dataset.');
	const url = event.url.searchParams;
	const page = Math.max(0, Number(url.get('page') ?? '0') || 0);
	const column = url.get('column') ?? '';
	const value = url.get('value') ?? '';
	const orgId = url.get('org') || undefined;
	const filters: Record<string, unknown> = {};
	if (column && value !== '') {
		const kind = dataset.columns.find((c) => c.name === column)?.type;
		// Typed values for typed columns.
		filters[column] =
			kind === 'integer' || kind === 'number'
				? Number(value)
				: kind === 'boolean'
					? value === 'true'
					: value;
	}
	const [rows, sync, orgs] = await Promise.all([
		client.POST('/api/v1/datasets/{id}/rows/query', {
			params: { path: { id } },
			body: { filters, orgId, limit: PAGE, offset: page * PAGE }
		}),
		client.GET('/api/v1/datasets/{id}/sync', { params: { path: { id } } }),
		client.GET('/api/v1/orgs')
	]);
	return {
		dataset,
		rows: rows.data ?? { rows: [], total: 0 },
		rowsError: rows.data ? null : problemMessage(rows.error, 'Could not read the rows.'),
		sync: sync.data ?? null,
		orgs: (orgs.data ?? []).map((o) => ({ id: o.id, name: o.name })),
		page,
		pageSize: PAGE,
		filter: { column, value, org: orgId ?? '' }
	};
};

function parseJson(text: string): { value: unknown } | { error: string } {
	try {
		return { value: JSON.parse(text) as unknown };
	} catch (e) {
		return { error: e instanceof Error ? e.message : 'not JSON' };
	}
}

export const actions: Actions = {
	addRows: async (event) => {
		const form = await event.request.formData();
		const parsed = parseJson(formString(form, 'rows'));
		if ('error' in parsed) return fail(400, { message: `Rows aren't JSON: ${parsed.error}` });
		const rows = Array.isArray(parsed.value) ? parsed.value : [parsed.value];
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/datasets/{id}/rows', {
			params: { path: { id: event.params.id } },
			body: {
				orgId: formString(form, 'orgId'),
				rows: rows as Record<string, unknown>[],
				replace: false
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not write the rows.')
			});
		}
		return { message: `${data.written} row(s) written.` };
	},
	saveSync: async (event) => {
		const form = await event.request.formData();
		const text = formString(form, 'sync').trim();
		let sync: unknown = null;
		if (text) {
			const parsed = parseJson(text);
			if ('error' in parsed) return fail(400, { message: `The sync isn't JSON: ${parsed.error}` });
			sync = parsed.value;
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/datasets/{id}/sync', {
			params: { path: { id: event.params.id } },
			// Checked by the API.
			body: { sync: sync as never }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the sync.')
			});
		}
		return { message: data.sync ? 'Sync saved.' : 'Sync stopped.' };
	},
	runSync: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/datasets/{id}/sync/run', {
			params: { path: { id: event.params.id } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not start it.') });
		}
		return { message: `Sync started: ${data.runs.length} run(s). See Runs for progress.` };
	}
};
