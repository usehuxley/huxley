import { error, fail } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { readFieldValues } from '#lib/fields/values.ts';
import { formString } from '#lib/forms.ts';
import { api, apiHeaders, apiUrl } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const path = { params: { path: { entity_type: 'organization', id } } };
	const [org, fields, tags, allTags, attachments, settings, modules, variables, sla, policies] =
		await Promise.all([
			client.GET('/api/v1/orgs/{id}', { params: { path: { id } } }),
			client.GET('/api/v1/entities/{entity_type}/{id}/fields', path),
			client.GET('/api/v1/entities/{entity_type}/{id}/tags', path),
			client.GET('/api/v1/tags'),
			client.GET('/api/v1/entities/{entity_type}/{id}/attachments', path),
			client.GET('/api/v1/orgs/{id}/settings', { params: { path: { id } } }),
			client.GET('/api/v1/orgs/{id}/modules', { params: { path: { id } } }),
			client.GET('/api/v1/orgs/{id}/variables', { params: { path: { id } } }),
			// Both 404 where the service desk is off.
			client.GET('/api/v1/orgs/{id}/sla', { params: { path: { id } } }),
			client.GET('/api/v1/servicedesk/sla-policies')
		]);
	if (org.response.status === 404) error(404, 'Organization not found.');
	const { user } = await event.parent();
	const healthModel =
		user.activeModules.includes('cx') && user.grantedPermissions.includes('crm:manage')
			? (await client.GET('/api/v1/orgs/{id}/health-model', { params: { path: { id } } })).data
			: undefined;
	if (!org.data || !fields.data || !tags.data) error(503, 'Could not load the organization.');
	return {
		org: org.data,
		fields: fields.data,
		tags: tags.data,
		allTags: allTags.data ?? [],
		attachments: attachments.data ?? [],
		settings: settings.data ?? [],
		modules: modules.data ?? [],
		variables: variables.data ?? null,
		sla: sla.data ?? null,
		slaPolicies: policies.data ?? [],
		healthModel: healthModel ?? null
	};
};

export const actions: Actions = {
	healthModel: async (event) => {
		const form = await event.request.formData();
		const weights: Record<string, number> = {};
		for (const key of ['csat', 'nps', 'sentiment', 'sla', 'reopens', 'volume', 'renewal']) {
			weights[key] = Number(formString(form, key)) || 0;
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/orgs/{id}/health-model', {
			params: { path: { id: event.params.id } },
			body: { weights }
		});
		if (!data) {
			return fail(response.status, {
				health: problemMessage(problem, 'Could not save the weights.')
			});
		}
		return { health: 'Weights saved. Scores use them from the next run.' };
	},
	fields: async (event) => {
		const form = await event.request.formData();
		const path = { entity_type: 'organization', id: event.params.id };
		const current = await api(event).GET('/api/v1/entities/{entity_type}/{id}/fields', {
			params: { path }
		});
		const values = readFieldValues(form, current.data ?? []);
		const { error: problem, response } = await api(event).PATCH(
			'/api/v1/entities/{entity_type}/{id}/fields',
			{ params: { path }, body: { values } }
		);
		if (response.status !== 200) {
			const errors: Record<string, string> = {};
			if (isApiProblem(problem)) {
				for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
			}
			return fail(response.status, {
				fieldErrors: errors,
				message: problemMessage(problem, 'Could not save the fields.')
			});
		}
		return { message: 'Saved.' };
	},

	setting: async (event) => {
		const form = await event.request.formData();
		const key = formString(form, 'key');
		const { error: problem, response } = await api(event).PUT('/api/v1/orgs/{id}/settings/{key}', {
			params: { path: { id: event.params.id, key } },
			body: { value: formString(form, 'value'), locked: form.get('locked') === 'on' }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the setting.')
			});
		}
		return { message: 'Setting saved.' };
	},

	clearSetting: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/orgs/{id}/settings/{key}',
			{ params: { path: { id: event.params.id, key: formString(form, 'key') } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not reset the setting.')
			});
		}
		return { message: 'Now inherited.' };
	},

	variable: async (event) => {
		const form = await event.request.formData();
		const key = formString(form, 'key').trim();
		const kind = formString(form, 'type') as 'string' | 'number' | 'bool' | 'json' | 'secret';
		const raw = formString(form, 'value');
		let value: unknown = raw;
		if (kind === 'number') value = Number(raw);
		if (kind === 'bool') value = ['true', 'yes', '1', 'on'].includes(raw.trim().toLowerCase());
		if (kind === 'json') {
			try {
				value = JSON.parse(raw);
			} catch {
				return fail(400, { message: 'The value must be valid JSON.' });
			}
		}
		const { error: problem, response } = await api(event).PUT('/api/v1/orgs/{id}/variables/{key}', {
			params: { path: { id: event.params.id, key } },
			body: { type: kind, value, locked: form.get('locked') === 'on' }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the variable.')
			});
		}
		return { message: `Saved ${key}.` };
	},

	clearVariable: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/orgs/{id}/variables/{key}',
			{ params: { path: { id: event.params.id, key: formString(form, 'key') } } }
		);
		if (response.status !== 204) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not remove the variable.')
			});
		}
		return { message: 'Removed.' };
	},

	sla: async (event) => {
		const form = await event.request.formData();
		const policyId = formString(form, 'policyId') || null;
		const { error: problem, response } = await api(event).PUT('/api/v1/orgs/{id}/sla', {
			params: { path: { id: event.params.id } },
			body: { policyId }
		});
		if (response.status !== 200) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not set the SLA policy.')
			});
		}
		return { message: policyId ? 'SLA policy set.' : 'Follows the policy above now.' };
	},
	module: async (event) => {
		const form = await event.request.formData();
		const key = formString(form, 'key');
		const enabled = form.get('enabled') === 'true';
		const { error: problem, response } = await api(event).PUT('/api/v1/orgs/{id}/modules/{key}', {
			params: { path: { id: event.params.id, key } },
			body: { enabled }
		});
		if (response.status !== 200) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not change the module.')
			});
		}
		return { message: enabled ? 'Turned back on here.' : 'Turned off here and below.' };
	},

	upload: async (event) => {
		const form = await event.request.formData();
		const file = form.get('file');
		if (!(file instanceof File) || file.size === 0) {
			return fail(400, { message: 'Choose a file to upload.' });
		}
		const body = new FormData();
		body.set('file', file, file.name);
		const response = await fetch(
			`${apiUrl}/api/v1/entities/organization/${encodeURIComponent(event.params.id)}/attachments`,
			{ method: 'POST', headers: apiHeaders(event), body }
		).catch(() => null);
		if (response?.status !== 201) {
			const problem: unknown = await response?.json().catch(() => undefined);
			return fail(response?.status ?? 503, {
				message: problemMessage(problem, 'Could not upload the file.')
			});
		}
		return { message: 'Uploaded.' };
	},

	deleteAttachment: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).DELETE('/api/v1/attachments/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (response.status !== 204) return fail(response.status, { message: 'Could not delete it.' });
		return { message: 'Deleted.' };
	},

	tag: async (event) => {
		const form = await event.request.formData();
		const path = {
			entity_type: 'organization',
			id: event.params.id,
			tag_id: formString(form, 'tagId')
		};
		const remove = formString(form, 'remove') === 'true';
		const { response } = remove
			? await api(event).DELETE('/api/v1/entities/{entity_type}/{id}/tags/{tag_id}', {
					params: { path }
				})
			: await api(event).PUT('/api/v1/entities/{entity_type}/{id}/tags/{tag_id}', {
					params: { path }
				});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not change the tag.' });
		return { message: remove ? 'Tag removed.' : 'Tag added.' };
	}
};
