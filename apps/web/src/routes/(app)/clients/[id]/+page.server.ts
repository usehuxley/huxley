import { error, fail, redirect } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { hoursFromForm } from '#lib/crm/hours.ts';
import { actionsFor } from '#lib/forms/actions.server.ts';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type AgreementInput = components['schemas']['AgreementInput'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const search = event.url.searchParams.get('q') ?? '';
	const path = { params: { path: { id } } };
	const [org, company, sites, contacts, actions] = await Promise.all([
		client.GET('/api/v1/orgs/{id}', path),
		client.GET('/api/v1/orgs/{id}/company', path),
		client.GET('/api/v1/orgs/{id}/sites', path),
		client.GET('/api/v1/orgs/{id}/contacts', {
			params: { path: { id }, query: { search: search || undefined, limit: 200 } }
		}),
		actionsFor(event, 'client', id)
	]);
	if (org.response.status === 404 || company.response.status === 404) {
		error(404, 'Client not found.');
	}
	if (!org.data || !company.data || !sites.data || !contacts.data) {
		error(503, 'Could not load the client.');
	}
	const { user } = await event.parent();
	const has = (module: string, permission: string) =>
		user.activeModules.includes(module) && user.grantedPermissions.includes(permission);
	const showAgreements = has('contracts', 'agreement:read');
	const [vault, documents, assets, agreements, workTypes, assetTypes, products, slaPolicies] =
		await Promise.all([
			has('vault', 'vault:read')
				? client.GET('/api/v1/vault/items', { params: { query: { orgId: id } } })
				: Promise.resolve({ data: undefined }),
			has('knowledge', 'kb:read')
				? client.GET('/api/v1/kb/articles', { params: { query: { orgId: id, limit: 200 } } })
				: Promise.resolve({ data: undefined }),
			has('cmdb', 'asset:read')
				? client.GET('/api/v1/assets', { params: { query: { orgId: id, limit: 500 } } })
				: Promise.resolve({ data: undefined }),
			showAgreements
				? client.GET('/api/v1/agreements', { params: { query: { orgId: id } } })
				: Promise.resolve({ data: undefined }),
			showAgreements ? client.GET('/api/v1/work-types') : Promise.resolve({ data: undefined }),
			showAgreements && user.activeModules.includes('cmdb')
				? client.GET('/api/v1/asset-types')
				: Promise.resolve({ data: undefined }),
			showAgreements && user.grantedPermissions.includes('catalog:read')
				? client.GET('/api/v1/catalog/products', { params: { query: { active: true } } })
				: Promise.resolve({ data: undefined }),
			showAgreements && user.activeModules.includes('servicedesk')
				? client.GET('/api/v1/servicedesk/sla-policies')
				: Promise.resolve({ data: undefined })
		]);
	const showPortal = has('portal', 'crm:read');
	const [portalAccounts, portalProviders] = await Promise.all([
		showPortal
			? client.GET('/api/v1/orgs/{id}/portal-accounts', { params: { path: { id } } })
			: Promise.resolve({ data: undefined }),
		showPortal
			? client.GET('/api/v1/orgs/{id}/portal-identity-providers', { params: { path: { id } } })
			: Promise.resolve({ data: undefined })
	]);
	// This month's usage of each active agreement.
	const usage = Object.fromEntries(
		await Promise.all(
			(agreements.data ?? [])
				.filter((a) => a.status === 'active')
				.map(async (a) => {
					const { data } = await client.GET('/api/v1/agreements/{id}/usage', {
						params: { path: { id: a.id } }
					});
					return [a.id, data ?? null] as const;
				})
		)
	);
	return {
		org: org.data,
		company: company.data,
		sites: sites.data,
		contacts: contacts.data,
		actions,
		vault: vault.data ?? null,
		documents: documents.data?.items ?? null,
		assets: assets.data?.items ?? null,
		portal: showPortal
			? {
					accounts: portalAccounts.data ?? [],
					providers: portalProviders.data ?? [],
					canManage: user.grantedPermissions.includes('crm:manage')
				}
			: null,
		agreements: agreements.data ?? null,
		agreementUsage: usage,
		workTypes: workTypes.data ?? [],
		assetTypes: (assetTypes.data ?? []).filter((t) => !t.archived),
		products: (products.data ?? []).filter((p) => p.kind !== 'bundle'),
		slaPolicies: (slaPolicies.data ?? []).map((p) => ({ id: p.id, name: p.name })),
		canManageAgreements: user.grantedPermissions.includes('agreement:manage'),
		canWriteAssets: user.grantedPermissions.includes('asset:write'),
		canWriteDocs: user.grantedPermissions.includes('kb:write'),
		canManageVault: user.grantedPermissions.includes('vault:manage'),
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
	const hours = hoursFromForm(form);
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

/** A vault item from its form. The secret is sent only when typed. */
function vaultBody(form: FormData) {
	const secret = formString(form, 'secret');
	return {
		name: formString(form, 'name'),
		kind: formString(form, 'kind') || 'login',
		username: formString(form, 'username'),
		url: formString(form, 'url'),
		notes: formString(form, 'notes'),
		tags: formString(form, 'tags')
			.split(',')
			.map((t) => t.trim())
			.filter(Boolean),
		...(secret ? { secret } : {})
	};
}

type ProviderInput = components['schemas']['PortalIdentityProviderInput'];

export const actions: Actions = {
	providerSave: async (event) => {
		const form = await event.request.formData();
		const providerId = formString(form, 'providerId');
		let body: ProviderInput;
		try {
			body = JSON.parse(formString(form, 'provider')) as ProviderInput;
		} catch {
			return fail(400, { portal: { message: 'The provider could not be read.' } });
		}
		const result = providerId
			? await api(event).PUT('/api/v1/portal-identity-providers/{id}', {
					params: { path: { id: providerId } },
					body
				})
			: await api(event).POST('/api/v1/orgs/{id}/portal-identity-providers', {
					params: { path: { id: event.params.id } },
					body
				});
		if (!result.data) {
			return fail(result.response.status, {
				portal: { message: problemMessage(result.error, 'Could not save the provider.') }
			});
		}
		return { portal: { done: 'Sign-in provider saved.' } };
	},
	providerDelete: async (event) => {
		const providerId = formString(await event.request.formData(), 'providerId');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/portal-identity-providers/{id}',
			{ params: { path: { id: providerId } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				portal: { message: problemMessage(problem, 'Could not delete it.') }
			});
		}
		return { portal: { done: 'Sign-in provider deleted.' } };
	},
	agreementSave: async (event) => {
		const form = await event.request.formData();
		const agreementId = formString(form, 'agreementId');
		let body: AgreementInput;
		try {
			body = JSON.parse(formString(form, 'agreement')) as AgreementInput;
		} catch {
			return fail(400, { agreement: { errors: {}, message: 'The agreement could not be read.' } });
		}
		const result = agreementId
			? await api(event).PUT('/api/v1/agreements/{id}', {
					params: { path: { id: agreementId } },
					body
				})
			: await api(event).POST('/api/v1/agreements', {
					body: { orgId: event.params.id, agreement: body }
				});
		if (!result.data) {
			return fail(result.response.status, {
				agreement: {
					errors: fieldErrors(result.error),
					message: problemMessage(result.error, 'Could not save the agreement.')
				}
			});
		}
		return { agreement: { saved: agreementId ? 'Agreement saved.' : 'Agreement added.' } };
	},
	agreementDelete: async (event) => {
		const agreementId = formString(await event.request.formData(), 'agreementId');
		const { error: problem, response } = await api(event).DELETE('/api/v1/agreements/{id}', {
			params: { path: { id: agreementId } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				agreement: { errors: {}, message: problemMessage(problem, 'Could not delete it.') }
			});
		}
		return { agreement: { saved: 'Agreement deleted.' } };
	},
	vaultSave: async (event) => {
		const form = await event.request.formData();
		const itemId = formString(form, 'itemId');
		const body = vaultBody(form);
		const result = itemId
			? await api(event).PATCH('/api/v1/vault/items/{id}', {
					params: { path: { id: itemId } },
					body: {
						version: Number(formString(form, 'version')),
						...body,
						clearSecret: form.get('clearSecret') === 'on'
					}
				})
			: await api(event).POST('/api/v1/vault/items', { body: { orgId: event.params.id, ...body } });
		if (!result.data) {
			return fail(result.response.status, {
				vault: {
					errors: fieldErrors(result.error),
					message: problemMessage(result.error, 'Could not save it.')
				}
			});
		}
		return { vault: { saved: true } };
	},
	vaultDelete: async (event) => {
		const itemId = formString(await event.request.formData(), 'itemId');
		const { error: problem, response } = await api(event).DELETE('/api/v1/vault/items/{id}', {
			params: { path: { id: itemId } }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				vault: { message: problemMessage(problem, 'Could not delete it.') }
			});
		}
		return { vault: { saved: true } };
	},
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
