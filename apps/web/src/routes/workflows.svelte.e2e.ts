import { expect, request, test, type Browser, type Page } from '@playwright/test';

// Full-stack: needs the API (with its workers) and a bootstrapped owner.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;

async function signIn(browser: Browser): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	return page;
}

function definition(key: string, output: string): string {
	return `apiVersion: usehuxley.io/v1
kind: Workflow
metadata:
  key: ${key}
  name: Greeter ${key}
spec:
  inputs:
    type: object
    required: [name]
    properties:
      name: { type: string }
  steps:
    # Still running when the run page loads: it must follow the run live.
    - id: pause
      type: wait
      duration: 1s
    - id: greet
      type: transform
      output: "${output}"
    - id: names
      type: foreach
      items: "{{ ['a', 'b'] }}"
      steps:
        - id: shout
          type: transform
          output: "{{ item | upper }}"
  outputs:
    greeting: "{{ steps.greet.output }}"
    letters: "{{ steps.names.output }}"
`;
}

test.describe('workflows', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);

	test('build, publish and run a workflow', async ({ browser }) => {
		const page = await signIn(browser);
		await page.getByRole('link', { name: 'Workflows' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Workflows');

		const key = `greeter-${Date.now().toString(36)}`;
		await page.getByLabel('Name').fill(`Greeter ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Greeter ${key}`);
		await expect(page.getByText('Not published', { exact: true })).toBeVisible();

		// A broken reference saves, and is reported.
		await page.getByRole('tab', { name: 'YAML' }).click();
		const editor = page.getByRole('textbox', { name: 'Workflow YAML' });
		await editor.fill(definition(key, 'Hi {{ steps.nope.output }}'));
		await page.getByRole('button', { name: 'Save draft' }).click();
		const problems = page.getByRole('list', { name: 'Problems' });
		await expect(problems).toContainText('there is no step "nope"');
		await expect(page.getByRole('button', { name: 'Publish version 1' })).toBeDisabled();

		// Fixed, saved and published.
		await editor.fill(definition(key, 'Hi {{ inputs.name }}'));
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await expect(problems).toHaveCount(0);
		await page.getByLabel('Release notes').fill('First version');
		await page.getByRole('button', { name: 'Publish version 1' }).click();
		await expect(page.getByText('Version 1 published')).toBeVisible();

		// Run it; the run page follows it to the end.
		await page.getByRole('textbox', { name: 'Inputs' }).fill('{"name": "Ada"}');
		await page.getByRole('button', { name: 'Run version 1' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Run');
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		const outputs = page.getByLabel('Run outputs');
		await expect(outputs).toContainText('"greeting": "Hi Ada"');
		await expect(outputs).toContainText('"B"');
		await expect(page.getByRole('list', { name: 'Steps' })).toContainText('shout[1]');

		// Back on the workflow, the run is listed.
		await page.getByRole('link', { name: `Greeter ${key}` }).click();
		await expect(
			page.getByRole('list', { name: 'Recent runs' }).getByText('Succeeded')
		).toBeVisible();

		// The published version and the draft are the same.
		await page.getByRole('link', { name: 'Compare versions' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Compare versions');
		await expect(page.getByText('No differences.')).toBeVisible();
	});

	test('workflows are organized in folders', async ({ browser }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const orgId = orgs.find((o) => o.parentId === null)!.id;
		for (const which of ['north', 'south']) {
			const key = `fold-${which}-${stamp}`;
			const made = await api.post('/api/v1/workflows', {
				data: {
					orgId,
					document: {
						apiVersion: 'usehuxley.io/v1',
						kind: 'Workflow',
						metadata: { key, name: `Folder test ${which} ${stamp}` },
						spec: { steps: [{ id: 'a', type: 'transform', output: 'x' }] }
					}
				}
			});
			expect(made.status(), await made.text()).toBe(201);
		}
		await api.dispose();

		const page = await signIn(browser);
		await page.goto('/workflows');
		const folders = page.getByRole('navigation', { name: 'Folders' });
		await expect(async () => {
			await folders.getByRole('button', { name: 'New folder' }).click();
			await expect(page.getByRole('dialog')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		await page.getByRole('dialog').getByLabel('Folder name').fill(`Ops ${stamp}`);
		await page.getByRole('dialog').getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Folder made.')).toBeVisible();

		// Move one workflow into it.
		await page.getByRole('button', { name: 'Move workflows' }).click();
		await page.getByLabel(`Choose Folder test north ${stamp}`, { exact: true }).check();
		await page.getByLabel('Move to').selectOption({ label: `Ops ${stamp}` });
		await page.getByRole('button', { name: 'Move', exact: true }).click();
		await expect(page.getByText('Workflow moved.')).toBeVisible();

		const list = page.getByRole('list', { name: 'Workflows' });
		await folders.getByRole('button', { name: new RegExp(`^Ops ${stamp}`) }).click();
		await expect(list.getByRole('link')).toHaveText([`Folder test north ${stamp}`]);
		// Search covers every folder.
		await page.getByLabel('Search workflows').fill(`folder test`);
		await expect(list.getByRole('link', { name: `Folder test south ${stamp}` })).toBeVisible();
		await expect(list.getByRole('link', { name: `Folder test north ${stamp}` })).toBeVisible();
		await page.getByLabel('Search workflows').fill('');

		// Deleting the folder moves its workflow back up.
		await page.getByRole('button', { name: 'Delete folder' }).click();
		await expect(page.getByText('Folder deleted.', { exact: false })).toBeVisible();
		await expect(folders).not.toContainText(`Ops ${stamp}`);
		await folders.getByRole('button', { name: /^Unfiled/ }).click();
		await expect(list.getByRole('link', { name: `Folder test north ${stamp}` })).toBeVisible();
	});

	test('workflows read organization variables', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		const word = `greeting_${suffix}`;

		// Set a variable and a secret on the root organization.
		await page.goto('/settings/organizations');
		await page.getByRole('link', { name: 'E2E MSP' }).first().click();
		await page.getByLabel('Variable', { exact: true }).fill(word);
		await page.getByLabel('Value', { exact: true }).fill('Howdy');
		await page.getByRole('button', { name: 'Set', exact: true }).click();
		await expect(page.getByRole('list', { name: 'Variables' })).toContainText(word);
		await page.getByLabel('Variable', { exact: true }).fill(`token_${suffix}`);
		await page.getByLabel('Type', { exact: true }).selectOption('secret');
		await page.getByLabel('Value', { exact: true }).fill('super-secret-value');
		await page.getByRole('button', { name: 'Set', exact: true }).click();
		const variables = page.getByRole('list', { name: 'Variables' });
		await expect(variables).toContainText(`{{ secrets.token_${suffix} }}`);
		await expect(variables).not.toContainText('super-secret-value');

		// A workflow in that organization reads it.
		await page.goto('/workflows');
		const key = `vars-${suffix}`;
		await page.getByLabel('Name').fill(`Greeter ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await page.getByRole('tab', { name: 'YAML' }).click();
		await page
			.getByRole('textbox', { name: 'Workflow YAML' })
			.fill(definition(key, `{{ vars.${word} }} {{ inputs.name }}`));
		await page.getByRole('button', { name: 'Save draft' }).click();
		// Saved before it runs: otherwise the run can take the earlier draft.
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Problems' })).toHaveCount(0);
		await page.getByRole('textbox', { name: 'Inputs' }).fill('{"name": "Ada"}');
		await page.getByRole('button', { name: 'Test run the draft' }).click();
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await expect(page.getByLabel('Run outputs')).toContainText('"greeting": "Howdy Ada"');
	});

	test('a failed run resumes after the cause is fixed', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		const flag = `flag_${suffix}`;
		await page.goto('/workflows');
		const key = `resume-${suffix}`;
		await page.getByLabel('Name').fill(`Greeter ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await page.getByRole('tab', { name: 'YAML' }).click();
		await page
			.getByRole('textbox', { name: 'Workflow YAML' })
			.fill(definition(key, `{{ vars.${flag}.upper() }} {{ inputs.name }}`));
		await page.getByRole('button', { name: 'Save draft' }).click();
		// Saved before it runs: otherwise the run can take the earlier draft.
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Problems' })).toHaveCount(0);
		await page.getByRole('textbox', { name: 'Inputs' }).fill('{"name": "Bo"}');
		await page.getByRole('button', { name: 'Test run the draft' }).click();
		await expect(page.getByText('Failed').first()).toBeVisible({ timeout: 20_000 });

		// Fix the cause (the variable is missing), then resume.
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as { id: string; name: string }[];
		const root = orgs.find((o) => o.name === 'E2E MSP')!;
		const set = await api.put(`/api/v1/orgs/${root.id}/variables/${flag}`, {
			data: { type: 'string', value: 'hey' }
		});
		expect(set.status()).toBe(204);
		await api.dispose();
		await page.getByRole('button', { name: 'Resume' }).click();
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await expect(page.getByLabel('Run outputs')).toContainText('"greeting": "HEY Bo"');

		await page.goto('/runs?status=succeeded');
		await expect(page.getByRole('list', { name: 'Runs' })).toContainText(`Greeter ${key}`);
	});

	test('build steps with the visual editor', async ({ browser }) => {
		const page = await signIn(browser);
		const key = `visual-${Date.now().toString(36)}`;
		await page.goto('/workflows');
		await page.getByLabel('Name').fill(`Visual ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Visual ${key}`);

		// The starter's step is a card; add an If after it.
		await expect(page.getByRole('button', { name: 'Step greet, Transform' })).toBeVisible();
		await page.getByRole('button', { name: 'Add a step after greet' }).click();
		await page.getByRole('menuitem', { name: /^If/ }).click();
		const settings = page.getByLabel('Step settings');
		await expect(settings).toContainText('Branch on a condition.');
		// Suggestions complete names inside {{ }}.
		const condition = settings.getByRole('combobox', { name: 'Condition' });
		await condition.fill('');
		await condition.pressSequentially('{{ inputs.na');
		await expect(page.getByRole('option', { name: /inputs\.name/ })).toBeVisible();
		await condition.press('Enter');
		await expect(condition).toHaveValue('{{ inputs.name');
		await condition.pressSequentially(" == 'Ada' }}");
		await condition.blur();

		// Inside Then: a transform that reads the first step.
		await page.getByRole('button', { name: 'Add a step at the start of Then' }).click();
		await page.getByRole('menuitem', { name: /^Transform/ }).click();
		await settings.getByLabel('Step id').fill('cheer');
		await settings.getByLabel('Step id').blur();
		await settings.getByLabel('Output').fill('{{ steps.greet.output ~ "!" }}');
		await settings.getByLabel('Output').blur();
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();

		// The canvas can flow left to right instead; the same steps show.
		const ltr = page.getByRole('radio', { name: 'Left to right' });
		await ltr.click();
		await expect(ltr).toHaveAttribute('aria-checked', 'true');
		await expect(page.getByRole('button', { name: 'Step cheer, Transform' })).toBeAttached();
		await page.getByRole('radio', { name: 'Top to bottom' }).click();

		// The YAML view shows the same document.
		await page.getByRole('tab', { name: 'YAML' }).click();
		const yaml = page.getByRole('textbox', { name: 'Workflow YAML' });
		await expect(yaml).toHaveValue(/type: if/);
		await expect(yaml).toHaveValue(/id: cheer/);

		await page.getByRole('textbox', { name: 'Inputs' }).fill('{"name": "Ada"}');
		await page.getByRole('button', { name: 'Test run the draft' }).click();
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await expect(page.getByRole('list', { name: 'Steps' })).toContainText('cheer');
	});

	test('triggered runs act as a chosen service account', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		const bot = `Runner ${suffix}`;
		await page.goto('/settings/service-accounts');
		await page.getByLabel('Name').fill(bot);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { name: bot })).toBeVisible();

		await page.goto('/workflows');
		const key = `runas-${suffix}`;
		await page.getByLabel('Name').fill(`Greeter ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Greeter ${key}`);

		await page.getByLabel('Service account').selectOption({ label: bot });
		await page.getByRole('button', { name: 'Save', exact: true }).click();
		await expect(page.getByText('Triggered runs now act as that account.')).toBeVisible();
		await page.reload();
		await expect(page.getByLabel('Service account')).toHaveValue(/.+/);
		await expect(page.getByLabel('Service account').locator('option:checked')).toHaveText(bot);
	});

	test('an approval waits for a decision', async ({ browser }) => {
		const page = await signIn(browser);
		const key = `approve-${Date.now().toString(36)}`;
		await page.goto('/workflows');
		await page.getByLabel('Name').fill(`Approve ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await page.getByRole('tab', { name: 'YAML' }).click();
		await page.getByRole('textbox', { name: 'Workflow YAML' }).fill(`apiVersion: usehuxley.io/v1
kind: Workflow
metadata: { key: ${key}, name: Approve ${key} }
spec:
  steps:
    - id: ask
      type: approval
      title: "Buy licenses for ${key}?"
      approvers: [role:owner]
  outputs:
    decision: "{{ steps.ask.output.decision }}"
    comment: "{{ steps.ask.output.comment }}"
`);
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await page.getByRole('button', { name: 'Test run the draft' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Run');
		const run = page.url();
		await expect(page.getByText('Waiting').first()).toBeVisible({ timeout: 20_000 });

		await page.getByRole('link', { name: 'Approvals' }).click();
		const card = page.getByRole('region', { name: 'Waiting for you' });
		await expect(card).toContainText(`Buy licenses for ${key}?`);
		await card.getByLabel('Comment (optional)').last().fill('Go ahead');
		await card.getByRole('button', { name: 'Approve' }).last().click();
		await expect(page.getByText('Approved.')).toBeVisible();

		await page.goto(run);
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await expect(page.getByLabel('Run outputs')).toContainText('"decision": "approved"');
		await expect(page.getByLabel('Run outputs')).toContainText('"comment": "Go ahead"');
	});

	test('a template becomes a draft, with its form', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		await page.goto('/workflows');
		await page.getByRole('link', { name: 'start from a template' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Templates');
		await page.getByRole('link', { name: /Request and notify/ }).click();
		await page.getByLabel('Name').fill(`Requests ${suffix}`);
		await page.getByLabel('Who to tell').fill('everyone');
		await page.getByRole('button', { name: 'Create draft' }).click();
		await expect(
			page.getByText('user:<id>, team:<id> or role:<key>, comma-separated').last()
		).toBeVisible();
		await page.getByLabel('Who to tell').fill('role:admin');
		await page.getByRole('button', { name: 'Create draft' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Requests ${suffix}`);
		// Its form is there to fill in once the workflow is published.
		await page.goto('/forms');
		await expect(page.getByRole('link', { name: 'Request', exact: true }).first()).toBeVisible();
	});

	test('a workflow saved as a template can be used again', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		await page.goto('/workflows');
		await page.getByLabel('Name').fill(`Original ${suffix}`);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Original ${suffix}`);
		await page.getByLabel('Template name').fill(`Reusable ${suffix}`);
		await page.getByRole('button', { name: 'Save as template' }).click();
		await expect(page.getByText(`Saved as the template “Reusable ${suffix}”`)).toBeVisible();

		await page.goto('/workflows/templates');
		await page.getByRole('link', { name: new RegExp(`Reusable ${suffix}`) }).click();
		await page.getByLabel('Name').fill(`Copy ${suffix}`);
		await page.getByRole('button', { name: 'Create draft' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Copy ${suffix}`);

		await page.goto('/workflows/templates');
		await page.getByRole('button', { name: `Delete Reusable ${suffix}` }).click();
		await expect(page.getByText('Template deleted.')).toBeVisible();
		await expect(page.getByRole('link', { name: new RegExp(`Reusable ${suffix}`) })).toHaveCount(0);
	});

	test('expressions preview with the last run', async ({ browser }) => {
		const page = await signIn(browser);
		const key = `preview-${Date.now().toString(36)}`;
		await page.goto('/workflows');
		await page.getByLabel('Name').fill(`Preview ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Preview ${key}`);

		// Before any run, there's nothing to preview with.
		await page.getByRole('button', { name: 'Step greet, Transform' }).click();
		const settings = page.getByLabel('Step settings');
		await settings.getByRole('button', { name: 'Preview with the last run' }).click();
		await expect(settings.getByRole('status')).toContainText('Run it once (a test run is enough)');

		// A test run, then the preview uses its data.
		await page.getByRole('textbox', { name: 'Inputs' }).fill('{"name": "Ada"}');
		await page.getByRole('button', { name: 'Test run the draft' }).click();
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await page.goBack();
		await page.getByRole('button', { name: 'Step greet, Transform' }).click();
		await settings.getByRole('button', { name: 'Preview with the last run' }).click();
		await expect(settings.getByLabel('Preview')).toHaveText('Hello Ada');
	});

	test('a run can be run again from a step', async ({ browser }) => {
		const page = await signIn(browser);
		const key = `rerun-${Date.now().toString(36)}`;
		await page.goto('/workflows');
		await page.getByLabel('Name').fill(`Rerun ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await page.getByRole('tab', { name: 'YAML' }).click();
		await page.getByRole('textbox', { name: 'Workflow YAML' }).fill(`apiVersion: usehuxley.io/v1
kind: Workflow
metadata: { key: ${key}, name: Rerun ${key} }
spec:
  inputs: { type: object, properties: { name: { type: string } } }
  steps:
    - { id: first, type: transform, output: "first {{ inputs.name }}" }
    - { id: second, type: transform, output: "{{ steps.first.output }}!" }
  outputs: { result: "{{ steps.second.output }}" }
`);
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await page.getByRole('textbox', { name: 'Inputs' }).fill('{"name": "Ada"}');
		await page.getByRole('button', { name: 'Test run the draft' }).click();
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		const firstRun = page.url();

		const steps = page.getByRole('list', { name: 'Steps' });
		await steps.getByText('second', { exact: true }).click();
		await page.getByRole('button', { name: 'Run again from here' }).click();
		await expect(page).not.toHaveURL(firstRun);
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await expect(page.getByLabel('Run outputs')).toContainText('"result": "first Ada!"');
	});
});
