import { error, fail, redirect } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { DAYS } from '#lib/crm/labels.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const search = event.url.searchParams.get('q') ?? '';
	const path = { params: { path: { id } } };
	const [org, company, sites, contacts] = await Promise.all([
		client.GET('/api/v1/orgs/{id}', path),
		client.GET('/api/v1/orgs/{id}/company', path),
		client.GET('/api/v1/orgs/{id}/sites', path),
		client.GET('/api/v1/orgs/{id}/contacts', {
			params: { path: { id }, query: { search: search || undefined, limit: 200 } }
		})
	]);
	if (org.response.status === 404 || company.response.status === 404) {
		error(404, 'Client not found.');
	}
	if (!org.data || !company.data || !sites.data || !contacts.data) {
		error(503, 'Could not load the client.');
	}
	return {
		org: org.data,
		company: company.data,
		sites: sites.data,
		contacts: contacts.data,
		search,
		tab: event.url.searchParams.get('tab') ?? 'overview'
	};
};

/** Field errors from a problem, keyed by field name. */
function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	if (isApiProblem(problem)) {
		for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
	}
	return errors;
}

/** A site from the site form: address, timezone and one range a day. */
function siteBody(form: FormData) {
	const hours: Record<string, string[][]> = {};
	for (const { key } of DAYS) {
		if (form.get(`open_${key}`) !== 'on') continue;
		hours[key] = [[formString(form, `from_${key}`), formString(form, `to_${key}`)]];
	}
	const version = formString(form, 'version');
	return {
		name: formString(form, 'name'),
		addressLine1: formString(form, 'addressLine1') || null,
		addressLine2: formString(form, 'addressLine2') || null,
		city: formString(form, 'city') || null,
		region: formString(form, 'region') || null,
		postalCode: formString(form, 'postalCode') || null,
		country: formString(form, 'country') || null,
		timezone: formString(form, 'timezone') || null,
		phone: formString(form, 'phone') || null,
		isPrimary: form.get('isPrimary') === 'on',
		hours,
		...(version ? { version: Number(version) } : {})
	};
}

export const actions: Actions = {
	company: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PUT('/api/v1/orgs/{id}/company', {
			params: { path: { id: event.params.id } },
			body: {
				relationship: formString(form, 'relationship'),
				status: formString(form, 'status'),
				website: formString(form, 'website') || null,
				phone: formString(form, 'phone') || null,
				notes: formString(form, 'notes') || null,
				version: Number(formString(form, 'version'))
			}
		});
		if (response.status !== 200) {
			return fail(response.status, {
				company: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not save the details.')
				}
			});
		}
		return { company: { saved: true } };
	},
	site: async (event) => {
		const form = await event.request.formData();
		const siteId = formString(form, 'id');
		const body = siteBody(form);
		const { error: problem, response } = siteId
			? await api(event).PUT('/api/v1/sites/{id}', {
					params: { path: { id: siteId } },
					body
				})
			: await api(event).POST('/api/v1/orgs/{id}/sites', {
					params: { path: { id: event.params.id } },
					body
				});
		if (response.status !== 200 && response.status !== 201) {
			return fail(response.status, {
				site: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not save the site.')
				}
			});
		}
		return { site: { saved: true } };
	},
	archiveSite: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE('/api/v1/sites/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				site: { errors: {}, message: problemMessage(problem, 'Could not archive the site.') }
			});
		}
		return { site: { archived: true } };
	},
	contact: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/orgs/{id}/contacts', {
			params: { path: { id: event.params.id } },
			body: {
				firstName: formString(form, 'firstName'),
				lastName: formString(form, 'lastName'),
				email: formString(form, 'email') || null,
				phone: formString(form, 'phone') || null,
				jobTitle: formString(form, 'jobTitle') || null
			}
		});
		if (!data) {
			return fail(response.status, {
				contact: {
					errors: fieldErrors(problem),
					message: problemMessage(problem, 'Could not add the contact.')
				}
			});
		}
		redirect(303, `/contacts/${data.id}`);
	}
};
