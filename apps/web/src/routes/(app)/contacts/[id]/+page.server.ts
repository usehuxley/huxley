import { error, fail, redirect } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { readFieldValues } from '#lib/fields/values.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const contact = await client.GET('/api/v1/contacts/{id}', { params: { path: { id } } });
	if (contact.response.status === 404) error(404, 'Contact not found.');
	if (!contact.data) error(503, 'Could not load the contact.');
	const [fields, sites, companies] = await Promise.all([
		client.GET('/api/v1/entities/{entity_type}/{id}/fields', {
			params: { path: { entity_type: 'contact', id } }
		}),
		client.GET('/api/v1/orgs/{id}/sites', { params: { path: { id: contact.data.orgId } } }),
		client.GET('/api/v1/companies', { params: { query: { providers: true } } })
	]);
	return {
		contact: contact.data,
		fields: fields.data ?? [],
		sites: sites.data ?? [],
		companies: companies.data ?? []
	};
};

function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	if (isApiProblem(problem)) {
		for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
	}
	return errors;
}

export const actions: Actions = {
	details: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT('/api/v1/contacts/{id}', {
			params: { path: { id: event.params.id } },
			body: {
				firstName: formString(form, 'firstName'),
				lastName: formString(form, 'lastName'),
				email: formString(form, 'email') || null,
				phone: formString(form, 'phone') || null,
				mobile: formString(form, 'mobile') || null,
				jobTitle: formString(form, 'jobTitle') || null,
				siteId: formString(form, 'siteId') || null,
				externalIds: formString(form, 'entra') ? { entra: formString(form, 'entra') } : {},
				version: Number(formString(form, 'version'))
			}
		});
		if (response.status !== 200) {
			return fail(response.status, {
				details: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not save the contact.')
				}
			});
		}
		return { details: { saved: true } };
	},
	companies: async (event) => {
		const form = await event.request.formData();
		const primary = formString(form, 'primary');
		const companies = form
			.getAll('orgId')
			.filter((v): v is string => typeof v === 'string' && v !== '')
			.map((orgId) => ({
				orgId,
				jobTitle: formString(form, `title_${orgId}`) || null,
				isPrimary: orgId === primary
			}));
		const added = formString(form, 'add');
		if (added && !companies.some((c) => c.orgId === added)) {
			companies.push({ orgId: added, jobTitle: null, isPrimary: false });
		}
		const { error: problem, response } = await api(event).PUT('/api/v1/contacts/{id}/companies', {
			params: { path: { id: event.params.id } },
			body: { companies, version: Number(formString(form, 'version')) }
		});
		if (response.status !== 200) {
			return fail(response.status, {
				companies: { message: problemMessage(problem, 'Could not change the companies.') }
			});
		}
		return { companies: { saved: true } };
	},
	fields: async (event) => {
		const form = await event.request.formData();
		const path = { entity_type: 'contact', id: event.params.id };
		const current = await api(event).GET('/api/v1/entities/{entity_type}/{id}/fields', {
			params: { path }
		});
		const values = readFieldValues(form, current.data ?? []);
		const { error: problem, response } = await api(event).PATCH(
			'/api/v1/entities/{entity_type}/{id}/fields',
			{ params: { path }, body: { values } }
		);
		if (response.status !== 200) {
			return fail(response.status, {
				fields: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not save the fields.')
				}
			});
		}
		return { fields: { saved: true } };
	},
	archive: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE('/api/v1/contacts/{id}', {
			params: { path: { id: event.params.id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				details: { errors: {}, message: problemMessage(problem, 'Could not archive the contact.') }
			});
		}
		redirect(303, `/clients/${formString(form, 'orgId')}?tab=contacts`);
	}
};
