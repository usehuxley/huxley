import { error, fail } from '@sveltejs/kit';
import { day, parseCsv } from '#lib/expenses/csv.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const status = (['unmatched', 'matched', 'ignored'] as const).find(
		(s) => s === event.url.searchParams.get('status')
	);
	const [cards, categories, orgs] = await Promise.all([
		client.GET('/api/v1/card-transactions', { params: { query: { status } } }),
		client.GET('/api/v1/expense-categories'),
		client.GET('/api/v1/orgs')
	]);
	if (cards.response.status === 404) error(404, 'Expenses are turned off.');
	if (!cards.data) error(503, 'Could not load card charges.');
	const { user } = await event.parent();
	return {
		cards: cards.data,
		categories: (categories.data ?? []).filter((c) => c.active && c.kind === 'expense'),
		providers: (orgs.data ?? [])
			.filter((o) => o.kind === 'provider')
			.map((o) => ({ value: o.id, label: o.name })),
		status: status ?? '',
		canImport: user.grantedPermissions.includes('expense:approve'),
		me: user.id
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	import: async (event) => {
		const form = await event.request.formData();
		const file = form.get('file');
		if (!(file instanceof File) || file.size === 0)
			return fail(400, { message: 'Choose a CSV file.' });
		const [header, ...lines] = parseCsv(await file.text());
		if (!header) return fail(400, { message: 'The file is empty.' });
		const col = (...names: string[]) =>
			header.findIndex((h) => names.includes(h.trim().toLowerCase()));
		const at = {
			date: col('date', 'posted', 'posted on', 'transaction date'),
			description: col('description', 'details', 'memo'),
			amount: col('amount', 'debit'),
			merchant: col('merchant', 'payee'),
			currency: col('currency'),
			card: col('card', 'card last 4', 'last4'),
			id: col('id', 'transaction id', 'reference'),
			email: col('email', 'cardholder', 'cardholder email')
		};
		if (at.date < 0 || at.description < 0 || at.amount < 0) {
			return fail(400, { message: 'The file needs date, description and amount columns.' });
		}
		const users = await api(event).GET('/api/v1/users', { params: { query: { limit: 200 } } });
		const byEmail = new Map((users.data?.items ?? []).map((u) => [u.email.toLowerCase(), u.id]));
		const cell = (row: string[], i: number) => (i >= 0 ? (row[i] ?? '').trim() : '');
		const rows = [];
		for (const [n, row] of lines.entries()) {
			const postedOn = day(cell(row, at.date));
			const amount = cell(row, at.amount).replace(/[$,]/g, '');
			if (!postedOn || !amount)
				return fail(400, { message: `Row ${n + 2}: a date and an amount, please.` });
			rows.push({
				postedOn,
				description: cell(row, at.description) || 'Card charge',
				merchant: cell(row, at.merchant) || null,
				amount,
				currency: cell(row, at.currency) || null,
				cardLast4: cell(row, at.card).slice(-4) || null,
				externalId: cell(row, at.id) || null,
				userId: byEmail.get(cell(row, at.email).toLowerCase()) ?? null
			});
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/card-transactions/import', {
			body: { orgId: formString(form, 'orgId'), rows }
		});
		if (!data) return failed(response.status, problem, 'Could not import them.');
		return {
			message: `${data.imported} imported (${data.matched} matched to expenses)${data.skipped ? `, ${data.skipped} already there` : ''}.`
		};
	},
	log: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/card-transactions/{id}/log', {
			params: { path: { id: formString(form, 'id') } },
			body: { categoryId: formString(form, 'categoryId') }
		});
		if (!data) return failed(response.status, problem, 'Could not log it.');
		return { message: 'Logged as your expense.' };
	},
	status: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/card-transactions/{id}/status', {
			params: { path: { id: formString(form, 'id') } },
			body: { ignored: formString(form, 'ignored') === 'yes' }
		});
		if (!data) return failed(response.status, problem, 'Could not change it.');
		return { message: data.status === 'ignored' ? 'Ignored.' : 'Unmatched.' };
	}
};
