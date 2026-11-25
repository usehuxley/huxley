import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { parseDuration } from '#lib/tickets/duration.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import { isoDay, mondayOf, weekDays } from '#lib/time/week.ts';
import type { Actions, PageServerLoad } from './$types';

const DAY = /^\d{4}-\d{2}-\d{2}$/;

export const load: PageServerLoad = async (event) => {
	const asked = event.url.searchParams.get('week') ?? '';
	const monday = mondayOf(DAY.test(asked) ? asked : isoDay(new Date()));
	const days = weekDays(monday);
	const client = api(event);
	const [entries, workTypes] = await Promise.all([
		client.GET('/api/v1/time-entries', { params: { query: { from: days[0], to: days[6] } } }),
		client.GET('/api/v1/work-types')
	]);
	if (entries.response.status === 404) error(404, 'The service desk is turned off.');
	if (!entries.data) error(503, 'Could not load your time.');
	return { monday, days, entries: entries.data, workTypes: workTypes.data ?? [] };
};

export const actions: Actions = {
	log: async (event) => {
		const form = await event.request.formData();
		const minutes = parseDuration(formString(form, 'duration'));
		if (minutes === null || Number.isNaN(minutes)) {
			return fail(400, {
				errors: { minutes: 'A duration, like 30m or 1h 15m.' },
				message: 'Not a duration.'
			});
		}
		const { error: problem, response } = await api(event).POST('/api/v1/time-entries', {
			body: {
				workTypeId: formString(form, 'workTypeId'),
				minutes,
				workedOn: formString(form, 'workedOn') || null,
				billable: form.get('billable') === 'on',
				notes: formString(form, 'notes') || null
			}
		});
		if (response.status !== 201) {
			return fail(response.status, {
				errors: fieldErrors(problem),
				message: problemMessage(problem, 'Could not log the time.')
			});
		}
		return { saved: true };
	},
	delete: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/time-entries/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				errors: {},
				message: problemMessage(problem, 'Could not delete it.')
			});
		}
		return { saved: true };
	}
};
