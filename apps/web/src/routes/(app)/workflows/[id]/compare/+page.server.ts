import { error } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { diffLines } from '#lib/workflows/diff.ts';
import { toYaml } from '#lib/workflows/yaml.ts';
import type { PageServerLoad } from './$types';

/** `draft`, or a version number. */
function side(value: string | null, fallback: string): string {
	return value === 'draft' || (value && /^\d+$/.test(value)) ? value : fallback;
}

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const [workflow, versions] = await Promise.all([
		client.GET('/api/v1/workflows/{id}', { params: { path: { id } } }),
		client.GET('/api/v1/workflows/{id}/versions', { params: { path: { id } } })
	]);
	if (workflow.response.status === 404) error(404, 'Workflow not found.');
	if (!workflow.data) error(503, 'Could not load the workflow.');
	const current = workflow.data.currentVersion;
	const from = side(event.url.searchParams.get('from'), current ? String(current) : 'draft');
	const to = side(event.url.searchParams.get('to'), 'draft');

	async function yamlOf(which: string): Promise<string> {
		if (which === 'draft') return toYaml(workflow.data!.draft);
		const version = await client.GET('/api/v1/workflows/{id}/versions/{version}', {
			params: { path: { id, version: Number(which) } }
		});
		if (!version.data) error(404, `Version ${which} not found.`);
		return toYaml(version.data.document);
	}
	const [before, after] = await Promise.all([yamlOf(from), yamlOf(to)]);
	return {
		workflow: { id, name: workflow.data.name },
		versions: versions.data ?? [],
		from,
		to,
		diff: diffLines(before, after)
	};
};
