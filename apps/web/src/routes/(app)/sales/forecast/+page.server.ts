import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { assignees } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

type Span = 'month' | 'quarter' | 'year';
const pad = (n: number) => String(n).padStart(2, '0');
const iso = (y: number, m: number, d: number) => `${y}-${pad(m)}-${pad(d)}`;
const lastDay = (y: number, m: number) => new Date(Date.UTC(y, m, 0)).getUTCDate();

/** The period `span` containing `at` (YYYY-MM-DD), moved `step` periods. */
function period(span: Span, at: string, step: number) {
	const [y0, m0] = at.split('-').map(Number);
	const months = span === 'month' ? 1 : span === 'quarter' ? 3 : 12;
	const first = span === 'year' ? 1 : span === 'quarter' ? Math.floor((m0 - 1) / 3) * 3 + 1 : m0;
	const index = y0 * 12 + (first - 1) + step * months;
	const y = Math.floor(index / 12);
	const m = (index % 12) + 1;
	const endIndex = index + months - 1;
	const ey = Math.floor(endIndex / 12);
	const em = (endIndex % 12) + 1;
	const label =
		span === 'year'
			? `${y}`
			: span === 'quarter'
				? `Q${Math.floor((m - 1) / 3) + 1} ${y}`
				: new Date(Date.UTC(y, m - 1, 1)).toLocaleDateString('en', {
						month: 'long',
						year: 'numeric',
						timeZone: 'UTC'
					});
	return { from: iso(y, m, 1), to: iso(ey, em, lastDay(ey, em)), label, months };
}

export const load: PageServerLoad = async (event) => {
	const params = event.url.searchParams;
	const span =
		(['month', 'quarter', 'year'] as const).find((s) => s === params.get('span')) ?? 'quarter';
	const today = new Date().toISOString().slice(0, 10);
	const at = /^\d{4}-\d{2}-\d{2}$/.test(params.get('at') ?? '') ? params.get('at')! : today;
	const p = period(span, at, 0);
	const client = api(event);
	const { user } = await event.parent();
	const canConfigure = user.grantedPermissions.includes('sales:configure');
	const [rows, people, orgs] = await Promise.all([
		client.GET('/api/v1/sales/forecast', { params: { query: { from: p.from, to: p.to } } }),
		canConfigure ? assignees(event) : null,
		canConfigure ? client.GET('/api/v1/orgs') : null
	]);
	if (rows.response.status === 404) error(404, 'Sales is turned off.');
	if (!rows.data) error(503, 'Could not load the forecast.');
	return {
		rows: rows.data,
		span,
		period: p,
		prev: period(span, at, -1).from,
		next: period(span, at, 1).from,
		people: people?.people ?? [],
		provider: (orgs?.data ?? []).find((o) => o.kind === 'provider')?.id ?? '',
		canConfigure
	};
};

export const actions: Actions = {
	quota: async (event) => {
		const form = await event.request.formData();
		const amount = formString(form, 'amount').trim();
		const { response, error: problem } = await api(event).PUT('/api/v1/sales/quotas', {
			body: {
				orgId: formString(form, 'orgId'),
				userId: formString(form, 'userId'),
				periodStart: formString(form, 'periodStart'),
				months: Number(formString(form, 'months')),
				amount: amount || null,
				currency: formString(form, 'currency') || 'USD'
			}
		});
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not set the quota.')
			});
		}
		return { message: amount ? 'Quota set.' : 'Quota removed.' };
	}
};
