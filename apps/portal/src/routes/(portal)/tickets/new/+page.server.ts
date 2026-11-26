import { fail, redirect } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

/** `?asset=` names the device it's about. */
export const load: PageServerLoad = async (event) => {
	const id = event.url.searchParams.get('asset');
	if (!id) return { device: null };
	const { data } = await api(event).GET('/api/portal/v1/devices/{id}', {
		params: { path: { id } }
	});
	return { device: data ?? null };
};

const text = (form: FormData, name: string) => {
	const v = form.get(name);
	return typeof v === 'string' ? v.trim() : '';
};

export const actions: Actions = {
	default: async (event) => {
		const form = await event.request.formData();
		const title = text(form, 'title');
		const description = text(form, 'description');
		const assetId = text(form, 'assetId') || undefined;
		if (!title) return fail(400, { title, description, message: 'Say briefly what you need.' });
		const { data, response } = await api(event).POST('/api/portal/v1/tickets', {
			body: { title, description, assetId }
		});
		if (!data) {
			return fail(response.status, {
				title,
				description,
				message: 'Your ticket couldn’t be raised. Try again.'
			});
		}
		redirect(303, `/tickets/${data.id}?raised=1`);
	}
};
