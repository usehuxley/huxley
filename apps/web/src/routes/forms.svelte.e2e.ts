import { expect, test, type Browser, type Page } from '@playwright/test';

// Full-stack: needs the API (with its workers) and a bootstrapped owner.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;

async function signIn(browser: Browser): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	return page;
}

test.describe('forms', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('a form starts the workflow that listens for it', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		const formName = `Laptop request ${suffix}`;
		const formKey = `laptop-request-${suffix}`;

		// Create the form and give it two fields.
		await page.getByRole('link', { name: 'Forms' }).click();
		await page.getByLabel('Name').fill(formName);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Edit form');
		await page.getByLabel('Schema').fill(
			JSON.stringify({
				type: 'object',
				required: ['name', 'seats'],
				properties: {
					name: { type: 'string', title: 'Who is it for?' },
					seats: { type: 'integer', title: 'How many?', minimum: 1 }
				}
			})
		);
		await expect(page.getByLabel('How many?')).toBeVisible();
		await page.getByRole('button', { name: 'Save', exact: true }).click();
		await expect(page.getByText('Saved.')).toBeVisible();

		// A workflow that listens for it.
		await page.goto('/workflows');
		await page.getByLabel('Name').fill(`Laptops ${suffix}`);
		await page.getByLabel('Key').fill(`laptops-${suffix}`);
		await page.getByRole('button', { name: 'Create' }).click();
		await page.getByRole('tab', { name: 'YAML' }).click();
		await page.getByRole('textbox', { name: 'Workflow YAML' }).fill(`apiVersion: usehuxley.com/v1
kind: Workflow
metadata: { key: laptops-${suffix}, name: Laptops ${suffix} }
spec:
  triggers: [ { type: form, form: ${formKey} } ]
  inputs:
    type: object
    required: [name, seats]
    properties: { name: { type: string }, seats: { type: integer } }
  steps:
    - { id: order, type: transform, output: "{{ inputs.seats }} for {{ inputs.name }}" }
  outputs: { order: "{{ steps.order.output }}" }
`);
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await page.getByRole('button', { name: 'Publish version 1' }).click();
		await expect(page.getByText('Version 1 published')).toBeVisible();

		// Fill it in: a bad answer is pointed out, then it goes through.
		await page.goto('/forms');
		await page.getByRole('link', { name: formName }).click();
		await page.getByLabel('Who is it for?').fill('Ada');
		await page.getByLabel('How many?').fill('0');
		await page.getByRole('button', { name: 'Submit' }).click();
		await expect(page.getByRole('alert').filter({ hasText: 'minimum' }).first()).toBeVisible();
		await page.getByLabel('How many?').fill('2');
		await page.getByRole('button', { name: 'Submit' }).click();
		await expect(page.getByText('Started 1 workflow run(s):')).toBeVisible();
		await page.getByRole('link', { name: 'View run' }).click();
		await expect(page.getByText('Succeeded').first()).toBeVisible({ timeout: 20_000 });
		await expect(page.getByLabel('Run outputs')).toContainText('"order": "2 for Ada"');
	});
});
