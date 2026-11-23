import { error, fail } from '@sveltejs/kit';
import type { components } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { Actions, PageServerLoad } from './$types';

type FieldType = components['schemas']['FieldType'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [fields, tags, orgs] = await Promise.all([
		client.GET('/api/v1/custom-fields', {
			params: { query: { entityType: 'organization', includeArchived: true } }
		}),
		client.GET('/api/v1/tags'),
		client.GET('/api/v1/orgs')
	]);
	if (!fields.data || !tags.data) error(503, 'Could not load fields and tags.');
	return {
		fields: fields.data,
		tags: tags.data,
		orgs: (orgs.data ?? []).filter((o) => o.status === 'active')
	};
};

export const actions: Actions = {
	createField: async (event) => {
		const form = await event.request.formData();
		const options = formString(form, 'options')
			.split('\n')
			.map((line) => line.trim())
			.filter(Boolean)
			.map((value) => ({ value, label: value }));
		const { error: problem, response } = await api(event).POST('/api/v1/custom-fields', {
			body: {
				entityType: 'organization',
				label: formString(form, 'label'),
				fieldType: formString(form, 'fieldType') as FieldType,
				help: formString(form, 'help'),
				options,
				required: form.get('required') === 'on',
				scopeOrgId: formString(form, 'scopeOrgId') || null
			}
		});
		if (response.status !== 201) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the field.')
			});
		}
		return { message: 'Field created.' };
	},

	archiveField: async (event) => {
		const form = await event.request.formData();
		const archived = formString(form, 'archived') === 'true';
		const { response } = await api(event).POST('/api/v1/custom-fields/{id}/archive', {
			params: { path: { id: formString(form, 'id') } },
			body: { archived }
		});
		if (response.status !== 200)
			return fail(response.status, { message: 'Could not change the field.' });
		return { message: archived ? 'Field archived.' : 'Field restored.' };
	},

	createTag: async (event) => {
		const form = await event.request.formData();
		const { error: problem, response } = await api(event).POST('/api/v1/tags', {
			body: {
				name: formString(form, 'name'),
				color: formString(form, 'color') || 'neutral',
				description: ''
			}
		});
		if (response.status !== 201) {
			return fail(response.status, {
				message: problemMessage(problem, 'Could not create the tag.')
			});
		}
		return { message: 'Tag created.' };
	},

	deleteTag: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).DELETE('/api/v1/tags/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (response.status !== 204)
			return fail(response.status, { message: 'Could not delete the tag.' });
		return { message: 'Tag deleted.' };
	}
};
