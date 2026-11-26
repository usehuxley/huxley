import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { keyFromName, starter } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [workflows, orgs, folders] = await Promise.all([
		client.GET('/api/v1/workflows'),
		client.GET('/api/v1/orgs'),
		client.GET('/api/v1/workflow-folders')
	]);
	if (!workflows.data) error(503, 'Could not load workflows.');
	return { workflows: workflows.data, orgs: orgs.data ?? [], folders: folders.data ?? [] };
};

/** An optional id from the form: empty means none. */
const optional = (form: FormData, name: string) => formString(form, name) || null;

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name').trim();
		const key = formString(form, 'key').trim() || keyFromName(name);
		const orgId = formString(form, 'orgId');
		if (!name || !orgId)
			return fail(400, { message: 'Give the workflow a name and an organization.' });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/workflows', {
			body: { orgId, document: starter(key, name) }
		});
		if (!data) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the workflow.')
			});
		}
		redirect(303, `/workflows/${data.id}`);
	},

	createFolder: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/workflow-folders', {
			body: {
				orgId: formString(form, 'orgId'),
				folder: { name: formString(form, 'folderName'), parentId: optional(form, 'parentId') }
			}
		});
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not make the folder.')
			});
		}
		return { done: 'Folder made.' };
	},

	updateFolder: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).PATCH('/api/v1/workflow-folders/{id}', {
			params: { path: { id: formString(form, 'id') } },
			body: { name: formString(form, 'folderName'), parentId: optional(form, 'parentId') }
		});
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not change the folder.')
			});
		}
		return { done: 'Folder saved.' };
	},

	deleteFolder: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).DELETE('/api/v1/workflow-folders/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not delete the folder.')
			});
		}
		return { done: 'Folder deleted. What was in it moved up a level.' };
	},

	move: async (event) => {
		const form = await event.request.formData();
		const workflowIds = form.getAll('workflowId').filter((v): v is string => typeof v === 'string');
		if (workflowIds.length === 0) return fail(400, { message: 'Choose workflows to move.' });
		const { error: problem, response } = await api(event).POST('/api/v1/workflow-folders/move', {
			body: { workflowIds, folderId: optional(form, 'folderId') }
		});
		if (!response.ok) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not move the workflows.')
			});
		}
		return { done: workflowIds.length === 1 ? 'Workflow moved.' : 'Workflows moved.' };
	}
};
