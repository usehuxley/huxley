import { expect, test, type Browser, type Page } from '@playwright/test';

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

test.describe('datasets', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('a dataset is created, filled, browsed and set to sync', async ({ browser }) => {
		const page = await signIn(browser);
		const name = `Staff ${Date.now().toString(36)}`;
		await page.getByRole('link', { name: 'Datasets' }).click();
		await page.getByLabel('Name', { exact: true }).fill(name);
		await page.getByLabel('Column 1').fill('upn');
		await page.getByRole('button', { name: 'Add column' }).click();
		await page.getByLabel('Column 2').fill('seats');
		await page.getByLabel('Type').nth(1).selectOption('integer');
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);
		await expect(page.getByText('No rows')).toBeVisible();

		await page
			.getByLabel('Rows (a JSON object or list)')
			.fill('[{"upn":"ada@x.test","seats":2},{"upn":"bob@x.test","seats":5}]');
		await page.getByRole('button', { name: 'Write rows' }).click();
		await expect(page.getByText('2 row(s) written.')).toBeVisible();
		const table = page.getByRole('table', { name: 'Dataset rows' });
		await expect(table).toContainText('ada@x.test');
		await expect(table).toContainText('bob@x.test');

		// Filters are typed: seats = 5.
		await page.getByLabel('Column', { exact: true }).selectOption('seats');
		await page.getByLabel('Equals').fill('5');
		await page.getByRole('button', { name: 'Filter' }).click();
		await expect(table).toContainText('bob@x.test');
		await expect(table).not.toContainText('ada@x.test');

		// Sync settings are checked, then saved with their next run.
		const sync = page.getByLabel('Sync settings (JSON; empty stops syncing)');
		await sync.fill('{"action":"nope","map":{"upn":"userPrincipalName"}}');
		await page.getByRole('button', { name: 'Save sync' }).click();
		await expect(page.getByRole('status').filter({ hasText: 'integration' })).toBeVisible();
		await sync.fill(
			'{"action":"msgraph/listUsers","map":{"upn":"userPrincipalName"},"schedule":{"interval":"6h"}}'
		);
		await page.getByRole('button', { name: 'Save sync' }).click();
		await expect(page.getByText('Sync saved.')).toBeVisible();
		await expect(page.getByText(/^Next: (?!never)/)).toBeVisible();
		await expect(page.getByRole('button', { name: 'Sync now' })).toBeVisible();
	});
});
