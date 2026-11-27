import { fail, type RequestEvent } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';

type Kind = components['schemas']['SalesActivityKind'];
type Refs = { opportunityId?: string; leadId?: string; contactId?: string };

/** A `datetime-local` value (local time) as RFC 3339, or null. */
function instant(value: string): string | null {
	if (!value) return null;
	const d = new Date(value);
	return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

/** Form action: adds an activity to the record. */
export async function addActivity(event: RequestEvent, refs: Refs) {
	const form = await event.request.formData();
	const kind = (formString(form, 'kind') || 'note') as Kind;
	const duration = formString(form, 'durationMinutes').trim();
	const { data, error, response } = await api(event).POST('/api/v1/activities', {
		body: {
			kind,
			subject: formString(form, 'subject'),
			body: formString(form, 'body'),
			...refs,
			ownerId: formString(form, 'ownerId') || null,
			dueAt: kind === 'task' ? instant(formString(form, 'when')) : null,
			startsAt: kind === 'meeting' ? instant(formString(form, 'when')) : null,
			durationMinutes: duration ? Number(duration) : null,
			outcome: formString(form, 'outcome') || null,
			done: kind === 'call' || (kind === 'meeting' && form.get('held') === 'on')
		}
	});
	if (!data) {
		return fail(response.status, { message: problemMessage(error, 'Could not add it.') });
	}
	return { message: 'Added.' };
}

/** Form action: marks an activity done, or not. */
export async function completeActivity(event: RequestEvent) {
	const form = await event.request.formData();
	const { data, error, response } = await api(event).POST('/api/v1/activities/{id}/done', {
		params: { path: { id: formString(form, 'activityId') } },
		body: { done: formString(form, 'done') === 'yes' }
	});
	if (!data) {
		return fail(response.status, { message: problemMessage(error, 'Could not change it.') });
	}
	return { message: data.doneAt ? 'Done.' : 'Opened again.' };
}
