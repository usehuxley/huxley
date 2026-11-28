import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

const VIEWS = ['infected', 'failed', 'pending'] as const;
type View = (typeof VIEWS)[number];

export const load: PageServerLoad = async (event) => {
	const asked = event.url.searchParams.get('show') ?? 'infected';
	const view: View = (VIEWS as readonly string[]).includes(asked) ? (asked as View) : 'infected';
	const client = api(event);
	const [status, files] = await Promise.all([
		client.GET('/api/v1/platform/file-scans/status'),
		client.GET('/api/v1/platform/file-scans', { params: { query: { status: view } } })
	]);
	if (status.response.status === 403) error(403, 'You can’t manage malware scanning.');
	if (!status.data || !files.data) error(503, 'Could not load malware scanning.');
	return { status: status.data, files: files.data, view };
};

export const actions: Actions = {
	rescan: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).POST(
			'/api/v1/platform/file-scans/{id}/rescan',
			{ params: { path: { id } } }
		);
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not scan it again.')
			});
		}
		return { message: 'Queued to be scanned again.' };
	},
	remove: async (event) => {
		const id = formString(await event.request.formData(), 'id');
		const { error: problem, response } = await api(event).DELETE(
			'/api/v1/platform/file-scans/{id}',
			{
				params: { path: { id } }
			}
		);
		if (!response.ok) {
			return fail(response.status, { message: problemMessage(problem, 'Could not delete it.') });
		}
		return { message: 'File deleted.' };
	}
};
