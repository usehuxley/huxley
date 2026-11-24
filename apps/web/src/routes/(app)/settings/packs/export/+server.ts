import { error } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { RequestHandler } from './$types';

const KINDS = ['integrations', 'workflows', 'forms', 'scripts'] as const;

/** Exports the chosen items as a pack and hands the file to the browser. */
export const POST: RequestHandler = async (event) => {
	const form = await event.request.formData();
	const ids = (kind: string) => form.getAll(kind).filter((v): v is string => typeof v === 'string');
	const body = {
		orgId: formString(form, 'orgId'),
		key: formString(form, 'key').trim(),
		name: formString(form, 'name').trim(),
		version: formString(form, 'version').trim(),
		description: formString(form, 'description').trim(),
		publisher: formString(form, 'publisher').trim(),
		...Object.fromEntries(KINDS.map((k) => [k, ids(k)]))
	};
	const response = await event.fetch(`${apiUrl}/api/v1/packs/export`, {
		method: 'POST',
		headers: { ...apiHeaders(event), 'content-type': 'application/json' },
		body: JSON.stringify(body)
	});
	if (!response.ok) {
		const problem: unknown = await response.json().catch(() => null);
		error(response.status, problemMessage(problem, 'Could not export the pack.'));
	}
	return new Response(response.body, {
		headers: {
			'content-type': 'application/octet-stream',
			'content-disposition':
				response.headers.get('content-disposition') ?? 'attachment; filename="pack.hxpack"'
		}
	});
};
