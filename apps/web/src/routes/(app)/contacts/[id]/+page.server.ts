import { error, fail, redirect } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { readFieldValues } from '#lib/fields/values.ts';
import { actionsFor } from '#lib/forms/actions.server.ts';
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
	const { user } = await event.parent();
	const portalOn =
		user.activeModules.includes('portal') && user.grantedPermissions.includes('crm:read');
	const [fields, sites, companies, actions, portalAccounts, nps] = await Promise.all([
		client.GET('/api/v1/entities/{entity_type}/{id}/fields', {
			params: { path: { entity_type: 'contact', id } }
		}),
		client.GET('/api/v1/orgs/{id}/sites', { params: { path: { id: contact.data.orgId } } }),
		client.GET('/api/v1/companies', { params: { query: { providers: true } } }),
		actionsFor(event, 'contact', contact.data.orgId),
		portalOn
			? client.GET('/api/v1/orgs/{id}/portal-accounts', {
					params: { path: { id: contact.data.orgId } }
				})
			: Promise.resolve({ data: undefined }),
		user.activeModules.includes('cx')
			? client.GET('/api/v1/contacts/{id}/nps', { params: { path: { id } } })
			: Promise.resolve({ data: undefined })
	]);
	return {
		contact: contact.data,
		fields: fields.data ?? [],
		sites: sites.data ?? [],
		companies: companies.data ?? [],
		actions,
		nps: nps.data ?? null,
		portal: portalOn
			? {
					account: (portalAccounts.data ?? []).find((a) => a.contactId === id) ?? null,
					canManage: user.grantedPermissions.includes('crm:manage')
				}
			: null
	};
};

function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	if (isApiProblem(problem)) {
		for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
	}
	return errors;
}

type PortalRole = 'client_user' | 'client_approver' | 'client_admin';
type PortalStatus = 'invited' | 'active' | 'disabled';
const ROLES: PortalRole[] = ['client_user', 'client_approver', 'client_admin'];
const STATUSES: PortalStatus[] = ['invited', 'active', 'disabled'];

export const actions: Actions = {
	portalInvite: async (event) => {
		const form = await event.request.formData();
		const role = formString(form, 'role') as PortalRole;
		if (!ROLES.includes(role)) return fail(400, { portal: { message: 'Choose a role.' } });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/contacts/{id}/portal-access', {
			params: { path: { id: event.params.id } },
			body: { role }
		});
		if (!data) {
			return fail(response.status, {
				portal: { message: problemMessage(problem, 'Could not invite them.') }
			});
		}
		return {
			portal: {
				done: data.invitationSent
					? 'Invited: a sign-in link is on its way to them.'
					: 'They have portal access. No email was sent: email or the portal’s address isn’t set up.'
			}
		};
	},
	portalUpdate: async (event) => {
		const form = await event.request.formData();
		const role = formString(form, 'role') as PortalRole;
		const status = formString(form, 'status') as PortalStatus;
		if (!ROLES.includes(role) || !STATUSES.includes(status)) {
			return fail(400, { portal: { message: 'Choose a role.' } });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PATCH('/api/v1/portal-accounts/{id}', {
			params: { path: { id: formString(form, 'accountId') } },
			body: { role, status, version: Number(formString(form, 'version')) || null }
		});
		if (!data) {
			return fail(response.status, {
				portal: { message: problemMessage(problem, 'Could not change it.') }
			});
		}
		return {
			portal: {
				done: data.status === 'disabled' ? 'Portal access disabled.' : 'Portal access saved.'
			}
		};
	},
	portalResend: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/portal-accounts/{id}/invitation', {
			params: { path: { id: formString(form, 'accountId') } }
		});
		if (!data) {
			return fail(response.status, {
				portal: { message: problemMessage(problem, 'Could not send it.') }
			});
		}
		return {
			portal: {
				done: data.invitationSent
					? 'A new sign-in link is on its way.'
					: 'No email was sent: email or the portal’s address isn’t set up.'
			}
		};
	},
	portalMfaReset: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/portal-accounts/{id}/mfa-reset', {
			params: { path: { id: formString(form, 'accountId') } }
		});
		if (!data) {
			return fail(response.status, {
				portal: { message: problemMessage(problem, 'Could not reset it.') }
			});
		}
		return { portal: { done: 'Two-factor sign-in reset. They’re signed out everywhere.' } };
	},
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
