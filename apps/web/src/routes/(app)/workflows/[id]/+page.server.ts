import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { fromYaml, toYaml } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const id = event.params.id;
	const params = { path: { id } };
	const [workflow, versions, triggers, runs] = await Promise.all([
		client.GET('/api/v1/workflows/{id}', { params }),
		client.GET('/api/v1/workflows/{id}/versions', { params }),
		client.GET('/api/v1/workflows/{id}/triggers', { params }),
		client.GET('/api/v1/runs', { params: { query: { workflowId: id, limit: 10 } } })
	]);
	if (workflow.response.status === 404) error(404, 'Workflow not found.');
	if (!workflow.data) error(503, 'Could not load the workflow.');
	return {
		workflow: workflow.data,
		yaml: toYaml(workflow.data.draft),
		versions: versions.data ?? [],
		triggers: triggers.data ?? [],
		runs: runs.data ?? [],
		// Webhooks are served on the same host (the proxy sends /hooks to the API).
		publicUrl: event.url.origin
	};
};

function inputsFrom(form: FormData): { inputs: Record<string, unknown> } | { message: string } {
	const text = formString(form, 'inputs').trim() || '{}';
	try {
		const inputs: unknown = JSON.parse(text);
		if (typeof inputs !== 'object' || inputs === null || Array.isArray(inputs)) {
			return { message: 'Inputs must be a JSON object.' };
		}
		return { inputs: inputs as Record<string, unknown> };
	} catch {
		return { message: 'Inputs must be valid JSON.' };
	}
}

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const yaml = formString(form, 'yaml');
		const parsed = fromYaml(yaml);
		if ('error' in parsed) return fail(400, { yaml, message: parsed.error });
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/workflows/{id}/draft', {
			params: { path: { id: event.params.id } },
			body: {
				revision: Number(formString(form, 'revision')),
				document: parsed.document as Record<string, unknown>
			}
		});
		if (!data) {
			return fail(response.status, {
				yaml,
				message: problemMessage(problem, 'Could not save the draft.')
			});
		}
		return {
			saved: true,
			message:
				data.diagnostics.length === 0
					? 'Draft saved. It is ready to publish.'
					: `Draft saved, with ${data.diagnostics.length} problem${data.diagnostics.length === 1 ? '' : 's'} to fix before publishing.`
		};
	},
	saveDocument: async (event) => {
		const form = await event.request.formData();
		let document: unknown;
		try {
			document = JSON.parse(formString(form, 'document'));
		} catch {
			return fail(400, { message: 'The workflow could not be read.' });
		}
		if (typeof document !== 'object' || document === null || Array.isArray(document)) {
			return fail(400, { message: 'The workflow could not be read.' });
		}
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/workflows/{id}/draft', {
			params: { path: { id: event.params.id } },
			body: {
				revision: Number(formString(form, 'revision')),
				document: document as Record<string, unknown>
			}
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the draft.')
			});
		}
		return {
			saved: true,
			message:
				data.diagnostics.length === 0
					? 'Draft saved. It is ready to publish.'
					: `Draft saved, with ${data.diagnostics.length} problem${data.diagnostics.length === 1 ? '' : 's'} to fix before publishing.`
		};
	},
	publish: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/workflows/{id}/publish', {
			params: { path: { id: event.params.id } },
			body: { notes: formString(form, 'notes') || null }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not publish the workflow.')
			});
		}
		return { message: `Published version ${data.version}.` };
	},
	run: async (event) => {
		const form = await event.request.formData();
		const parsed = inputsFrom(form);
		if ('message' in parsed) return fail(400, { runMessage: parsed.message });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/workflows/{id}/runs', {
			params: { path: { id: event.params.id } },
			body: { inputs: parsed.inputs, test: formString(form, 'test') === 'true' }
		});
		if (!data) {
			return fail(response.status, {
				runMessage: problemMessage(problem, 'Could not start the run.')
			});
		}
		redirect(303, `/runs/${data.id}`);
	},
	archive: async (event) => {
		const { response, error: problem } = await api(event).POST('/api/v1/workflows/{id}/archive', {
			params: { path: { id: event.params.id } }
		});
		if (!response.ok) {
			return fail(response.status, { message: problemMessage(problem, 'Could not archive it.') });
		}
		return { message: 'Archived. It no longer runs or answers its webhooks.' };
	},
	restore: async (event) => {
		const { response, error: problem } = await api(event).POST('/api/v1/workflows/{id}/restore', {
			params: { path: { id: event.params.id } }
		});
		if (!response.ok) {
			return fail(response.status, { message: problemMessage(problem, 'Could not restore it.') });
		}
		return { message: 'Restored.' };
	}
};
