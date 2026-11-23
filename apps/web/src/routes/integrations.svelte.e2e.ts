import { expect, test, type Browser, type Page } from '@playwright/test';

// Full-stack: needs the API and a bootstrapped owner.
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

function spec(title: string): string {
	return `openapi: 3.0.3
info: { title: "${title}", version: "1.0" }
servers: [{ url: "https://devices.example.com/v1" }]
components:
  securitySchemes:
    token: { type: http, scheme: bearer }
security: [{ token: [] }]
paths:
  /devices:
    get:
      operationId: listDevices
      summary: List devices
      tags: [Devices]
      responses: { "200": { description: ok } }
  /devices/{id}/reboot:
    post:
      summary: Reboot a device
      tags: [Devices]
      responses: { "202": { description: queued } }
`;
}

test.describe('integrations', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('import an OpenAPI spec and see its actions', async ({ browser }) => {
		const page = await signIn(browser);
		await page.getByRole('link', { name: 'Integrations' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Integrations');

		const title = `Devices ${Date.now().toString(36)}`;
		await page.getByLabel('Or paste the spec').fill(spec(title));
		await page.getByRole('button', { name: 'Import', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(title);

		const actions = page.getByRole('list', { name: 'Devices actions' });
		await expect(actions).toContainText('List devices');
		await expect(actions).toContainText('post_devices_id_reboot');
		await expect(page.getByText('Bearer token', { exact: true })).toBeVisible();
		// The undeclared {id} is noted, not fatal.
		await expect(page.getByRole('list', { name: 'Import warnings' })).toContainText(
			"path parameter {id} isn't declared"
		);

		// A connection with a bearer token: the token is never shown again.
		const connections = page.getByRole('form', { name: 'New connection' });
		await connections.getByLabel('Connection name').fill('Lab devices');
		await connections.getByLabel('Token').fill('t0ken-never-shown');
		await connections.getByRole('button', { name: 'Create connection' }).click();
		await expect(page.getByText('Connection Lab devices created.')).toBeVisible();
		const list = page.getByRole('list', { name: 'Connections' });
		await expect(list).toContainText('Lab devices');
		await expect(list).toContainText('token set');
		await expect(list).toContainText('Ready');
		await expect(page.locator('body')).not.toContainText('t0ken-never-shown');

		// A workflow calls one of its actions, picked in the visual editor.
		const integrationPage = page.url();
		await page.goto('/workflows');
		const key = `devices-flow-${Date.now().toString(36)}`;
		await page.getByLabel('Name').fill(`Devices ${key}`);
		await page.getByLabel('Key').fill(key);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Devices ${key}`);
		await page.getByRole('button', { name: 'Add a step after greet' }).click();
		await page.getByRole('menuitem', { name: /^Integration action/ }).click();
		const settings = page.getByLabel('Step settings');
		await settings.getByLabel('Action').selectOption({ label: 'List devices' });
		await expect(settings).toContainText('GET /devices');
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByText('Draft saved. It is ready to publish.')).toBeVisible();
		await page.goto(integrationPage);

		// A newer spec is reviewed first; dropping an action is flagged.
		const newer = spec(title)
			.replace('"1.0"', '"1.1"')
			.replace(/ {2}\/devices\/\{id\}\/reboot:[\s\S]*$/, '');
		await page.getByLabel('Or paste the newer spec').fill(newer);
		await page.getByRole('button', { name: 'Review changes' }).click();
		const review = page.getByRole('region', { name: 'Update review' });
		await expect(review).toContainText('can break workflows or connections');
		await expect(review).toContainText('post_devices_id_reboot');
		await expect(page.getByRole('list', { name: 'Versions' })).not.toContainText('Version 2');
		await review.getByRole('button', { name: 'Apply as a new version' }).click();
		await expect(page.getByText('Imported as version 2.')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Versions' })).toContainText('Version 2');

		// Roll back: version 1 is current again, with its reboot action.
		await page.getByRole('button', { name: 'Make current' }).click();
		await expect(page.getByText('Version 1 is current.')).toBeVisible();
		await expect(actions).toContainText('post_devices_id_reboot');
	});
});
