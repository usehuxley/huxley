import { error } from '@sveltejs/kit';
import { deleteExpense, logExpense, uploadReceipt } from '#lib/expenses/server.ts';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const params = event.url.searchParams;
	const status = (['draft', 'submitted', 'approved', 'rejected', 'reimbursed'] as const).find(
		(s) => s === params.get('status')
	);
	const { user } = await event.parent();
	const everyone =
		params.get('who') === 'everyone' && user.grantedPermissions.includes('expense:read');
	const [entries, categories, orgs] = await Promise.all([
		client.GET('/api/v1/expenses', {
			params: { query: { status, userId: everyone ? undefined : user.id } }
		}),
		client.GET('/api/v1/expense-categories'),
		client.GET('/api/v1/orgs')
	]);
	if (entries.response.status === 404) error(404, 'Expenses are turned off.');
	if (!entries.data) error(entries.response.status === 403 ? 403 : 503, 'Could not load expenses.');
	const all = orgs.data ?? [];
	return {
		entries: entries.data,
		categories: (categories.data ?? []).filter((c) => c.active),
		targets: [
			...all
				.filter((o) => o.kind === 'provider')
				.map((o) => ({ value: o.id, label: `${o.name} (internal)` })),
			...all
				.filter((o) => o.kind === 'client')
				.sort((a, b) => a.name.localeCompare(b.name))
				.map((o) => ({ value: o.id, label: o.name }))
		],
		status: status ?? '',
		everyone,
		canReadAll: user.grantedPermissions.includes('expense:read'),
		canWrite: user.grantedPermissions.includes('expense:write'),
		me: user.id
	};
};

export const actions: Actions = {
	logExpense: (event) => logExpense(event, 'org', null),
	uploadReceipt: (event) => uploadReceipt(event),
	deleteExpense: (event) => deleteExpense(event)
};
