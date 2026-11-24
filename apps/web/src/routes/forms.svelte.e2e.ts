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
		await page.getByRole('button', { name: 'JSON', exact: true }).click();
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
		await page.getByRole('textbox', { name: 'Workflow YAML' }).fill(`apiVersion: usehuxley.io/v1
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

	test('the builder makes a form without JSON', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		const formName = `Onboarding ${suffix}`;
		await page.goto('/forms');
		await page.getByLabel('Name').fill(formName);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Edit form');

		// Start from nothing: drop the starter field.
		const remove = page.getByRole('button', { name: 'Remove field' });
		while ((await remove.count()) > 0) await remove.first().click();

		// A required text field, and a choice.
		await page.getByRole('button', { name: 'Add field' }).click();
		const first = page.getByRole('group', { name: 'Field 1' });
		await first.getByLabel('Label').fill('Full name');
		await expect(first.getByLabel('Name in workflows')).toHaveValue('full_name');
		await first.getByLabel('Required').check();
		await page.getByRole('button', { name: 'Add field' }).click();
		const second = page.getByRole('group', { name: 'Field 2' });
		await second.getByLabel('Label').fill('Department');
		await second.getByLabel('Type').selectOption('select');
		// No choices yet: it can't be saved.
		await expect(second.getByRole('alert')).toHaveText('Add at least one choice.');
		await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
		await second.getByLabel('Choices, one per line').fill('Sales\nSupport');
		// Department first.
		await second.getByRole('button', { name: 'Move up' }).click();
		await expect(page.getByRole('group', { name: 'Field 1' }).getByLabel('Label')).toHaveValue(
			'Department'
		);
		await page.getByRole('button', { name: 'Save', exact: true }).click();
		await expect(page.getByText('Saved.')).toBeVisible();

		// The JSON behind it.
		await page.getByRole('button', { name: 'JSON', exact: true }).click();
		const schema = JSON.parse(await page.getByLabel('Schema').inputValue()) as {
			required: string[];
			properties: Record<string, unknown>;
		};
		expect(schema.required).toEqual(['full_name']);
		expect(schema.properties.department).toEqual({
			type: 'string',
			title: 'Department',
			enum: ['Sales', 'Support']
		});

		// People see it in that order.
		await page.goto('/forms');
		await page.getByRole('link', { name: formName }).click();
		await expect(page.getByLabel('Department')).toBeVisible();
		await expect(page.getByLabel('Full name *')).toBeVisible();
	});

	test('a question shows only when an earlier answer calls for it', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		const formName = `Equipment ${suffix}`;
		await page.goto('/forms');
		await page.getByLabel('Name').fill(formName);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Edit form');
		const remove = page.getByRole('button', { name: 'Remove field' });
		while ((await remove.count()) > 0) await remove.first().click();

		await page.getByRole('button', { name: 'Add field' }).click();
		const first = page.getByRole('group', { name: 'Field 1' });
		await first.getByLabel('Label').fill('Needs a laptop');
		await first.getByLabel('Type').selectOption('boolean');
		await page.getByRole('button', { name: 'Add field' }).click();
		const second = page.getByRole('group', { name: 'Field 2' });
		await second.getByLabel('Label').fill('Laptop model');
		await second.getByLabel('Required').check();
		await second.getByLabel('Show only when').selectOption('needs_a_laptop');
		await second.getByLabel('…is (comma-separated; empty: answered)').fill('true');
		// The preview follows the rule.
		await expect(page.locator('#preview-laptop_model')).toBeHidden();
		await page.locator('#preview-needs_a_laptop').check();
		await expect(page.locator('#preview-laptop_model')).toBeVisible();
		await page.getByRole('button', { name: 'Save', exact: true }).click();
		await expect(page.getByText('Saved.')).toBeVisible();

		// Filling it in: hidden, then asked, and not required while hidden.
		await page.goto('/forms');
		await page.getByRole('link', { name: formName }).click();
		await expect(page.getByLabel('Laptop model *')).toBeHidden();
		await page.getByRole('button', { name: 'Submit' }).click();
		await expect(
			page.getByText('Saved. No workflows are listening for this form yet.')
		).toBeVisible();
	});

	test('anyone with a public link can fill a form in', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		await page.goto('/forms');
		await page.getByLabel('Name').fill(`Visitor ${suffix}`);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Edit form');
		await page.getByLabel('Link name').fill('Website');
		await page.getByRole('button', { name: 'Make a link' }).click();
		const url = (await page.getByTestId('form-link').textContent())!.trim();
		expect(url).toContain('/f/hxf_');

		// A visitor, signed out.
		const visitor = await (await browser.newContext()).newPage();
		await visitor.goto(url);
		await expect(visitor.getByRole('heading', { level: 1 })).toHaveText(`Visitor ${suffix}`);
		await visitor.getByRole('textbox').first().fill('Ada');
		await visitor.getByRole('button', { name: 'Send' }).click();
		await expect(visitor.getByText('Thanks, your answers were sent.')).toBeVisible();

		// Turned off: the link stops working.
		await expect(page.getByText('last used never')).toBeVisible();
		await page.reload();
		await page.getByRole('button', { name: 'Turn off' }).click();
		await expect(page.getByText('The link no longer works.')).toBeVisible();
		const gone = await visitor.goto(url);
		expect(gone?.status()).toBe(404);
	});
});
