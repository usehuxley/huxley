import { error, fail } from '@sveltejs/kit';
import type { components } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api, apiHeaders, apiUrl } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

type Review = components['schemas']['Review'];

/** The largest pack the API accepts. */
const MAX_PACK_BYTES = 20 * 1024 * 1024;

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const orgs = (await client.GET('/api/v1/orgs')).data ?? [];
	const orgId = event.url.searchParams.get('org') ?? orgs[0]?.id;
	if (!orgId) error(404, 'No organization to install packs in.');
	const { data, response } = await client.GET('/api/v1/packs', { params: { query: { orgId } } });
	if (response.status === 403 || response.status === 404) {
		error(403, 'You do not have access to packs here.');
	}
	if (!data) error(503, 'Could not load packs.');
	// What can be exported from here (lists you can't see come back empty).
	const [builtin, integrations, workflows, forms, scripts] = await Promise.all([
		client.GET('/api/v1/packs/builtin'),
		client.GET('/api/v1/integrations'),
		client.GET('/api/v1/workflows'),
		client.GET('/api/v1/forms'),
		client.GET('/api/v1/scripts')
	]);
	const here = <T extends { orgId: string; id: string; key: string; name: string }>(items?: T[]) =>
		(items ?? [])
			.filter((i) => i.orgId === orgId)
			.map((i) => ({ id: i.id, key: i.key, name: i.name }));
	return {
		installed: data,
		builtin: builtin.data ?? [],
		orgId,
		orgs: orgs.map((o) => ({ id: o.id, name: o.name })),
		exportable: {
			integrations: here(integrations.data),
			workflows: here(workflows.data),
			forms: here(forms.data),
			scripts: here(scripts.data)
		}
	};
};

export const actions: Actions = {
	upload: async (event) => {
		const form = await event.request.formData();
		const orgId = formString(form, 'orgId');
		const file = form.get('pack');
		if (!(file instanceof File) || file.size === 0) {
			return fail(400, { message: 'Choose a .hxpack file.' });
		}
		if (file.size > MAX_PACK_BYTES) {
			return fail(413, { message: 'Packs are at most 20 MB.' });
		}
		// A raw body: the typed client only sends JSON.
		const response = await event.fetch(
			`${apiUrl}/api/v1/packs/uploads?orgId=${encodeURIComponent(orgId)}`,
			{
				method: 'POST',
				headers: { ...apiHeaders(event), 'content-type': 'application/octet-stream' },
				body: await file.arrayBuffer()
			}
		);
		const body: unknown = await response.json().catch(() => null);
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(body as never, 'That pack could not be read.')
			});
		}
		return { review: body as Review, orgId };
	},
	reviewBuiltin: async (event) => {
		const form = await event.request.formData();
		const orgId = formString(form, 'orgId');
		const [owner = '', name = ''] = formString(form, 'key').split('/');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/packs/builtin/{owner}/{name}', {
			params: { path: { owner, name }, query: { orgId } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not review it.') });
		}
		return { review: data, orgId };
	},
	install: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'uploadId');
		const orgId = formString(form, 'orgId');
		const version = formString(form, 'version');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/packs/uploads/{id}/install', {
			params: { path: { id } },
			body: { orgId, version }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not install it.') });
		}
		return {
			message: `Installed ${data.key} ${data.version}. Its workflows are drafts: review and publish them.`
		};
	}
};
