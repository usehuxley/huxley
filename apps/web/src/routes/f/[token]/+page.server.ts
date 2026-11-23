import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

/** A form opened through a public link: no sign-in. */
export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/public/forms/{token}', {
		params: { path: { token: event.params.token } }
	});
	if (response.status === 429) error(429, 'Too many requests. Wait a minute and try again.');
	if (!data) error(404, 'This link doesn’t work. It may have been turned off or expired.');
	return { form: data };
};

export const actions: Actions = {
	default: async (event) => {
		let answers: Record<string, unknown>;
		try {
			answers = JSON.parse(formString(await event.request.formData(), 'answers') || '{}') as Record<
				string,
				unknown
			>;
		} catch {
			return fail(400, { message: 'The answers couldn’t be read.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/public/forms/{token}', {
			params: { path: { token: event.params.token } },
			body: { answers }
		});
		if (data) return { received: true };
		const errors: Record<string, string> = {};
		const list = (problem as { errors?: { pointer?: string; message?: string }[] } | undefined)
			?.errors;
		for (const e of list ?? []) {
			const name = e.pointer?.replace(/^answers\//, '');
			if (name && e.message) errors[name] = e.message;
		}
		return fail(response.status, {
			message: problemMessage(problem, 'It couldn’t be sent.'),
			errors
		});
	}
};
