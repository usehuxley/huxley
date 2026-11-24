import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { keyFromName } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

const TYPES = [
	'text',
	'integer',
	'number',
	'boolean',
	'timestamp',
	'date',
	'json',
	'uuid'
] as const;
type ColumnType = (typeof TYPES)[number];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [datasets, orgs] = await Promise.all([
		client.GET('/api/v1/datasets'),
		client.GET('/api/v1/orgs')
	]);
	if (!datasets.data) error(503, 'Could not load datasets.');
	return { datasets: datasets.data, orgs: orgs.data ?? [] };
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name').trim();
		const orgId = formString(form, 'orgId');
		const names = form.getAll('columnName').map((v) => (typeof v === 'string' ? v.trim() : ''));
		const types = form.getAll('columnType').map((v) => (typeof v === 'string' ? v : 'text'));
		const keys = new Set(form.getAll('columnKey').map(String));
		const columns = names
			.map((n, i) => ({
				name: n,
				type: (TYPES.includes(types[i] as ColumnType) ? types[i] : 'text') as ColumnType
			}))
			.filter((c) => c.name);
		const primaryKey = names.filter((n, i) => n && keys.has(String(i)));
		if (!name || !orgId || columns.length === 0) {
			return fail(400, {
				message: 'Give the dataset a name, an organization and at least one column.'
			});
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/datasets', {
			body: { orgId, dataset: { key: keyFromName(name), name, columns, primaryKey } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not create it.') });
		}
		redirect(303, `/datasets/${data.id}`);
	}
};
