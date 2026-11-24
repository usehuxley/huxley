import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [skills, certifications] = await Promise.all([
		client.GET('/api/v1/workforce/skills'),
		client.GET('/api/v1/workforce/certifications')
	]);
	if (skills.response.status === 404) error(404, 'Workforce is turned off.');
	if (!skills.data || !certifications.data) error(503, 'Could not load the catalogs.');
	return { skills: skills.data, certifications: certifications.data };
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

const version = (form: FormData) => {
	const v = formString(form, 'version');
	return v ? { version: Number(v) } : {};
};

export const actions: Actions = {
	skill: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const body = {
			name: formString(form, 'name'),
			category: formString(form, 'category') || null,
			description: formString(form, 'description') || null,
			...version(form)
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/workforce/skills/{id}', { params: { path: { id } }, body })
			: await api(event).POST('/api/v1/workforce/skills', { body });
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the skill.');
		return { saved: true };
	},
	archiveSkill: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/workforce/skills/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return failed(response.status, problem, 'Could not archive it.');
		return { saved: true };
	},
	certification: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const months = formString(form, 'validityMonths');
		const body = {
			vendor: formString(form, 'vendor'),
			name: formString(form, 'name'),
			code: formString(form, 'code') || null,
			validityMonths: months ? Number(months) : null,
			description: formString(form, 'description') || null,
			...version(form)
		};
		const { error: problem, response } = id
			? await api(event).PUT('/api/v1/workforce/certifications/{id}', {
					params: { path: { id } },
					body
				})
			: await api(event).POST('/api/v1/workforce/certifications', { body });
		if (response.status !== 200 && response.status !== 201)
			return failed(response.status, problem, 'Could not save the certification.');
		return { saved: true };
	},
	archiveCertification: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/workforce/certifications/{id}',
			{ params: { path: { id } } }
		);
		if (response.status !== 204) return failed(response.status, problem, 'Could not archive it.');
		return { saved: true };
	}
};
