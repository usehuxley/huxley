import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { connectionFromForm } from '#lib/integrations/connections.ts';
import { specFrom } from '#lib/integrations/spec.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const params = { path: { id: event.params.id } };
	const [integration, versions, connections, orgs] = await Promise.all([
		client.GET('/api/v1/integrations/{id}', { params }),
		client.GET('/api/v1/integrations/{id}/versions', { params }),
		client.GET('/api/v1/connections', {
			params: { query: { integrationId: event.params.id } }
		}),
		client.GET('/api/v1/orgs')
	]);
	if (integration.response.status === 404) error(404, 'Integration not found.');
	if (!integration.data) error(503, 'Could not load the integration.');
	return {
		signedIn: event.url.searchParams.get('signedIn'),
		integration: integration.data,
		versions: versions.data ?? [],
		connections: connections.data ?? [],
		orgs: orgs.data ?? []
	};
};

export const actions: Actions = {
	/** Shows what a newer spec would change; nothing is saved yet. */
	preview: async (event) => {
		const spec = await specFrom(await event.request.formData());
		if (!spec.trim()) return fail(400, { message: 'Give the new spec (paste or upload).' });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/integrations/{id}/preview', {
			params: { path: { id: event.params.id } },
			body: { spec }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not read it.') });
		}
		return { preview: data, spec };
	},
	/** Applies a previewed spec, if the integration hasn't changed since. */
	update: async (event) => {
		const form = await event.request.formData();
		const spec = formString(form, 'spec');
		const baseVersion = Number(formString(form, 'baseVersion')) || null;
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/integrations/{id}/spec', {
			params: { path: { id: event.params.id } },
			body: { spec, baseVersion }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not apply it.') });
		}
		return { message: `Imported as version ${data.currentVersion}.` };
	},
	connect: async (event) => {
		const form = await event.request.formData();
		const client = api(event);
		const integration = await client.GET('/api/v1/integrations/{id}', {
			params: { path: { id: event.params.id } }
		});
		const scheme = integration.data?.definition.authSchemes.find(
			(s) => s.key === formString(form, 'authScheme')
		);
		const connection = connectionFromForm(form, scheme?.type);
		if (!connection.name) return fail(400, { connectMessage: 'Give the connection a name.' });
		const {
			data,
			error: problem,
			response
		} = await client.POST('/api/v1/connections', {
			body: {
				orgId: formString(form, 'orgId'),
				integrationId: event.params.id,
				connection
			}
		});
		if (!data) {
			return fail(response.status, {
				connectMessage: problemMessage(problem, 'Could not create the connection.')
			});
		}
		return { message: `Connection ${data.name} created.` };
	},
	/** Sends the browser to the provider to sign the connection in. */
	signIn: async (event) => {
		const id = formString(await event.request.formData(), 'connectionId');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/connections/{id}/sign-in', {
			params: { path: { id } }
		});
		if (!data) {
			return fail(response.status, {
				connectMessage: problemMessage(problem, 'Could not start the sign-in.')
			});
		}
		redirect(303, data.authorizeUrl);
	},
	disconnect: async (event) => {
		const id = formString(await event.request.formData(), 'connectionId');
		const { response, error: problem } = await api(event).DELETE('/api/v1/connections/{id}', {
			params: { path: { id } }
		});
		if (!response.ok) {
			return fail(response.status, {
				connectMessage: problemMessage(problem, 'Could not delete it.')
			});
		}
		return { message: 'Connection deleted.' };
	},
	editAction: async (event) => {
		const form = await event.request.formData();
		const key = formString(form, 'key');
		const client = api(event);
		const params = { path: { id: event.params.id } };
		const current = await client.GET('/api/v1/integrations/{id}', { params });
		if (!current.data) return fail(404, { message: 'Integration not found.' });
		const actions = { ...(current.data.overlay.actions ?? {}) };
		actions[key] = {
			name: formString(form, 'name') || null,
			group: formString(form, 'group') || null,
			description: formString(form, 'description') || null,
			hidden: form.get('hidden') === 'on'
		};
		const {
			data,
			error: problem,
			response
		} = await client.PUT('/api/v1/integrations/{id}/overlay', { params, body: { actions } });
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not save it.') });
		}
		return { message: `Saved your changes to ${key}.` };
	},
	setCurrent: async (event) => {
		const version = Number(formString(await event.request.formData(), 'version'));
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/integrations/{id}/current', {
			params: { path: { id: event.params.id } },
			body: { version }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not switch.') });
		}
		return { message: `Version ${version} is current.` };
	}
};
