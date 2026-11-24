import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fieldErrors } from '#lib/tickets/server.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [person, skills, certifications] = await Promise.all([
		client.GET('/api/v1/workforce/people/{user_id}', {
			params: { path: { user_id: event.params.id } }
		}),
		client.GET('/api/v1/workforce/skills'),
		client.GET('/api/v1/workforce/certifications')
	]);
	if (person.response.status === 404) error(404, 'Person not found.');
	if (person.response.status === 403) error(403, 'You can’t see other people’s records.');
	if (!person.data) error(503, 'Could not load this person.');
	return {
		person: person.data,
		skills: skills.data ?? [],
		certifications: certifications.data ?? []
	};
};

const failed = (status: number, problem: unknown, fallback: string) =>
	fail(status, { errors: fieldErrors(problem), message: problemMessage(problem, fallback) });

/** A held certification from the form. */
function heldBody(form: FormData) {
	return {
		certificationId: formString(form, 'certificationId') || null,
		earnedOn: formString(form, 'earnedOn'),
		expiresOn: formString(form, 'expiresOn') || null,
		credentialId: formString(form, 'credentialId') || null,
		verifyUrl: formString(form, 'verifyUrl') || null
	};
}

export const actions: Actions = {
	skill: async (event) => {
		const form = await event.request.formData();
		const level = formString(form, 'level');
		const { error: problem, response } = await api(event).PUT(
			'/api/v1/workforce/people/{user_id}/skills/{skill_id}',
			{
				params: { path: { user_id: event.params.id, skill_id: formString(form, 'skillId') } },
				body: { level: level ? Number(level) : null, verified: form.get('verified') === 'true' }
			}
		);
		if (response.status !== 200)
			return failed(response.status, problem, 'Could not save the skill.');
		return { saved: 'skill' };
	},
	addCertification: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST(
			'/api/v1/workforce/people/{user_id}/certifications',
			{ params: { path: { user_id: event.params.id } }, body: heldBody(form) }
		);
		if (response.status !== 201)
			return failed(response.status, problem, 'Could not add the certification.');
		return { saved: 'certification' };
	},
	updateCertification: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT(
			'/api/v1/workforce/people/{user_id}/certifications/{held_id}',
			{
				params: { path: { user_id: event.params.id, held_id: formString(form, 'heldId') } },
				body: heldBody(form)
			}
		);
		if (response.status !== 200)
			return failed(response.status, problem, 'Could not save the certification.');
		return { saved: 'certification' };
	},
	removeCertification: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/workforce/people/{user_id}/certifications/{held_id}',
			{ params: { path: { user_id: event.params.id, held_id: formString(form, 'heldId') } } }
		);
		if (response.status !== 200)
			return failed(response.status, problem, 'Could not remove the certification.');
		return { saved: 'removed' };
	}
};
