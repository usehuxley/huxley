import { error, fail, redirect } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { keyFromName } from '#lib/workflows/yaml.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [scripts, orgs] = await Promise.all([
		client.GET('/api/v1/scripts'),
		client.GET('/api/v1/orgs')
	]);
	if (!scripts.data) error(503, 'Could not load scripts.');
	return { scripts: scripts.data, orgs: orgs.data ?? [] };
};

const STARTERS = {
	pwsh: 'param(\n    [Parameter(Mandatory)] [string] $Name\n)\n\nWrite-Output "Hello $Name"\n',
	python: 'import json, sys\n\nparams = json.load(sys.stdin)\nprint(f"Hello {params[\'name\']}")\n',
	javascript:
		'export default async ({ name }) => {\n\tconsole.log(`Hello ${name}`);\n\treturn { greeting: `Hello ${name}` };\n};\n'
};

export const actions: Actions = {
	create: async (event) => {
		const form = await event.request.formData();
		const name = formString(form, 'name').trim();
		const orgId = formString(form, 'orgId');
		const chosen = formString(form, 'language');
		const language = chosen === 'python' || chosen === 'javascript' ? chosen : 'pwsh';
		if (!name || !orgId)
			return fail(400, { message: 'Give the script a name and an organization.' });
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/scripts', {
			body: {
				orgId,
				script: {
					key: keyFromName(name),
					name,
					language,
					source: STARTERS[language],
					parameters: {
						type: 'object',
						required: ['name'],
						properties: { name: { type: 'string', title: 'Name' } }
					}
				}
			}
		});
		if (!data) {
			return fail(response.status, { message: problemMessage(problem, 'Could not create it.') });
		}
		redirect(303, `/scripts/${data.id}`);
	}
};
