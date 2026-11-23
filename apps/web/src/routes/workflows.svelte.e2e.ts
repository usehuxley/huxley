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
	return `apiVersion: usehuxley.com/v1
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
		const editor = page.getByRole('textbox', { name: 'Workflow YAML' });
		await editor.fill(definition(key, 'Hi {{ steps.nope.output }}'));
		await page.getByRole('button', { name: 'Save draft' }).click();
		const problems = page.getByRole('list', { name: 'Problems' });
		await expect(problems).toContainText('there is no step "nope"');
		await expect(page.getByRole('button', { name: 'Publish version 1' })).toBeDisabled();

		// Fixed, saved and published.
		await editor.fill(definition(key, 'Hi {{ inputs.name }}'));
		await page.getByRole('button', { name: 'Save draft' }).click();
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
		await page
			.getByRole('textbox', { name: 'Workflow YAML' })
			.fill(definition(key, `{{ vars.${word} }} {{ inputs.name }}`));
		await page.getByRole('button', { name: 'Save draft' }).click();
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
		await page
			.getByRole('textbox', { name: 'Workflow YAML' })
			.fill(definition(key, `{{ vars.${flag}.upper() }} {{ inputs.name }}`));
		await page.getByRole('button', { name: 'Save draft' }).click();
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
});
