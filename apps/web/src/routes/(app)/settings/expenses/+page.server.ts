import { fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [categories, rates] = await Promise.all([
		client.GET('/api/v1/expense-categories'),
		client.GET('/api/v1/mileage-rates')
	]);
	return { categories: categories.data ?? [], rates: rates.data ?? [] };
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

export const actions: Actions = {
	saveCategory: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const kind: 'mileage' | 'expense' =
			formString(form, 'kind') === 'mileage' ? 'mileage' : 'expense';
		const body = {
			name: formString(form, 'name'),
			kind,
			billable: form.get('billable') === 'on',
			reimbursable: form.get('reimbursable') === 'on',
			markupPct: formString(form, 'markupPct').trim() || null,
			limitAmount: formString(form, 'limitAmount').trim() || null,
			receiptRequired: form.get('receiptRequired') === 'on',
			active: form.get('active') === 'on'
		};
		const client = api(event);
		const { data, error, response } = id
			? await client.PUT('/api/v1/expense-categories/{id}', { params: { path: { id } }, body })
			: await client.POST('/api/v1/expense-categories', { body });
		if (!data) return failed(response.status, error, 'Could not save the category.');
		return { message: 'Saved.' };
	},
	setRate: async (event) => {
		const form = await event.request.formData();
		const { data, error, response } = await api(event).PUT('/api/v1/mileage-rates', {
			body: {
				unit: formString(form, 'unit') || 'mi',
				rate: formString(form, 'rate'),
				currency: formString(form, 'currency') || 'USD',
				effectiveFrom: formString(form, 'effectiveFrom')
			}
		});
		if (!data) return failed(response.status, error, 'Could not set the rate.');
		return { message: 'Rate set.' };
	},
	deleteRate: async (event) => {
		const form = await event.request.formData();
		const { response, error } = await api(event).DELETE('/api/v1/mileage-rates/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (!response.ok) return failed(response.status, error, 'Could not remove it.');
		return { message: 'Removed.' };
	}
};
