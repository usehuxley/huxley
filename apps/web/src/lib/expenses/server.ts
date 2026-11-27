import { fail, type RequestEvent } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api, apiHeaders, apiUrl } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

export type ExpenseRef = 'ticket' | 'project' | 'org';
type User = { activeModules: string[]; grantedPermissions: string[]; id: string };

/** Expenses on a ticket or project, and the categories to log one with. */
export async function expensesFor(
	event: RequestEvent,
	ref: 'ticket' | 'project',
	refId: string,
	user: User
) {
	if (!user.activeModules.includes('expenses')) return null;
	const canWrite = user.grantedPermissions.includes('expense:write');
	const canRead = user.grantedPermissions.includes('expense:read');
	if (!canWrite && !canRead) return null;
	const client = api(event);
	const query = ref === 'ticket' ? { ticketId: refId } : { projectId: refId };
	const [entries, categories] = await Promise.all([
		client.GET('/api/v1/expenses', { params: { query } }),
		client.GET('/api/v1/expense-categories')
	]);
	return {
		entries: entries.data ?? [],
		categories: (categories.data ?? []).filter((c) => c.active),
		canWrite,
		me: user.id
	};
}

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { message: problemMessage(problem, fallback) });

/** Form action: logs an expense against the ticket, the project, or (from the form) a client. */
export async function logExpense(event: RequestEvent, ref: ExpenseRef, refId: string | null) {
	const form = await event.request.formData();
	const num = (k: string) => formString(form, k).trim() || null;
	const target =
		ref === 'ticket'
			? { ticketId: refId }
			: ref === 'project'
				? { projectId: refId, phaseId: formString(form, 'phaseId') || null }
				: { orgId: formString(form, 'orgId') || null };
	const { data, error, response } = await api(event).POST('/api/v1/expenses', {
		body: {
			categoryId: formString(form, 'categoryId'),
			spentOn: formString(form, 'spentOn'),
			description: formString(form, 'description'),
			vendor: formString(form, 'vendor') || null,
			amount: num('amount'),
			tax: num('tax'),
			distance: num('distance'),
			unit: formString(form, 'unit') || null,
			billable: form.get('billable') === 'on',
			reimbursable: form.get('reimbursable') === 'on',
			...target
		}
	});
	if (!data) return failed(response.status, error, 'Could not log it.');
	const file = form.get('receipt');
	if (file instanceof File && file.size > 0) {
		const sent = await sendReceipt(event, data.id, file);
		if (!sent.ok)
			return fail(sent.status, { message: 'Logged, but the receipt could not be kept.' });
	}
	return { message: 'Logged.' };
}

async function sendReceipt(event: RequestEvent, id: string, file: File) {
	const body = new FormData();
	body.append('file', file, file.name);
	return fetch(`${apiUrl}/api/v1/expenses/${encodeURIComponent(id)}/receipt`, {
		method: 'PUT',
		headers: apiHeaders(event),
		body
	});
}

/** Form action: keeps a receipt on an expense. */
export async function uploadReceipt(event: RequestEvent) {
	const form = await event.request.formData();
	const file = form.get('receipt');
	if (!(file instanceof File) || file.size === 0) {
		return fail(400, { message: 'Choose a photo or PDF of the receipt.' });
	}
	const res = await sendReceipt(event, formString(form, 'id'), file);
	if (!res.ok) {
		const problem: unknown = await res.json().catch(() => null);
		return failed(res.status, problem, 'Could not keep the receipt.');
	}
	return { message: 'Receipt kept.' };
}

/** Form action: takes what AI read from the receipt into the draft. */
export async function applyReading(event: RequestEvent) {
	const form = await event.request.formData();
	const { data, error, response } = await api(event).POST('/api/v1/expenses/{id}/apply-reading', {
		params: { path: { id: formString(form, 'id') } }
	});
	if (!data) return failed(response.status, error, 'Could not take it.');
	return { message: 'Updated from the receipt.' };
}

/** Form action: deletes a draft expense. */
export async function deleteExpense(event: RequestEvent) {
	const form = await event.request.formData();
	const { response, error } = await api(event).DELETE('/api/v1/expenses/{id}', {
		params: { path: { id: formString(form, 'id') } }
	});
	if (!response.ok) return failed(response.status, error, 'Could not delete it.');
	return { message: 'Deleted.' };
}
