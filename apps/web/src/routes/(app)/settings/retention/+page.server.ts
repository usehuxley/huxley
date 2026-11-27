import { fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/v1/platform/retention');
	return { retention: data ?? null };
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const n = (k: string) => Number(formString(form, k));
		const { data, error, response } = await api(event).PUT('/api/v1/platform/retention', {
			body: {
				runMonths: n('runMonths'),
				eventDays: n('eventDays'),
				notificationDays: n('notificationDays'),
				aiCallDays: n('aiCallDays')
			}
		});
		if (!data) return fail(response.status, { message: problemMessage(error, 'Could not save.') });
		return { message: 'Saved.' };
	},
	sweep: async (event) => {
		const { data, error, response } = await api(event).POST('/api/v1/platform/retention/sweep');
		if (!data)
			return fail(response.status, { message: problemMessage(error, 'The sweep failed.') });
		return {
			message: `Removed ${data.runs} runs, ${data.events} events, ${data.notifications} notifications and ${data.aiCalls} AI call records.`
		};
	}
};
