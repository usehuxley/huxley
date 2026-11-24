import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const run = await client.GET('/api/v1/runs/{id}', { params: { path: { id: event.params.id } } });
	if (run.response.status === 404) error(404, 'Run not found.');
	if (!run.data) error(503, 'Could not load the run.');
	const workflow = run.data.workflowId
		? await client.GET('/api/v1/workflows/{id}', {
				params: { path: { id: run.data.workflowId } }
			})
		: null;
	// Top-level steps of the current draft: a test run can start at one.
	const draftSteps = (
		workflow?.data?.draft as { spec?: { steps?: { id?: unknown }[] } } | undefined
	)?.spec?.steps;
	const topLevel = (draftSteps ?? [])
		.map((s) => s.id)
		.filter((id): id is string => typeof id === 'string');
	return {
		run: run.data,
		workflow: workflow?.data ? { id: workflow.data.id, name: workflow.data.name } : null,
		topLevel
	};
};

export const actions: Actions = {
	rerunFrom: async (event) => {
		const form = await event.request.formData();
		const workflowId = formString(form, 'workflowId');
		const stepId = formString(form, 'stepId');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/workflows/{id}/runs', {
			params: { path: { id: workflowId } },
			body: { test: true, inputs: {}, from: { runId: event.params.id, stepId } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not start it.') });
		}
		redirect(303, `/runs/${data.id}`);
	},
	resume: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/runs/{id}/resume', {
			params: { path: { id: event.params.id } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not resume it.') });
		}
		return {
			message: data.resumed ? 'Resuming from the step that failed.' : 'Only failed runs resume.'
		};
	},
	cancel: async (event) => {
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/runs/{id}/cancel', {
			params: { path: { id: event.params.id } }
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not cancel it.') });
		}
		return {
			message: data.cancelled ? 'Cancelling: nothing new will start.' : 'It had already finished.'
		};
	}
};
