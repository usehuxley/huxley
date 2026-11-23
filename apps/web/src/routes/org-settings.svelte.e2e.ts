import { expect, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

async function openOrg(page: Page, name: string) {
	await page.goto('/settings/organizations');
	await page.getByRole('link', { name, exact: true }).click();
	await expect(page.getByRole('heading', { name: 'Organization settings' })).toBeVisible();
}

test.describe('organization settings', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('a value locked at the MSP flows down and can be released', async ({ page }) => {
		const client = `Northwind ${Date.now().toString(36)}`;

		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');

		await page.goto('/settings/organizations');
		await page.getByRole('button', { name: 'Add' }).first().click();
		await page.getByLabel('Name').fill(client);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByText(client)).toBeVisible();

		// Set and lock the ticket prefix at the MSP.
		await openOrg(page, 'E2E MSP');
		const msp = page.getByTestId('setting-numbering-ticket_prefix');
		await msp.getByRole('textbox', { name: 'Ticket prefix' }).fill('MSP');
		await msp.getByLabel('Lock').check();
		await msp.getByRole('button', { name: 'Save Ticket prefix' }).click();
		await expect(msp).toContainText('Set here and locked below');

		// The client sees it, locked.
		await openOrg(page, client);
		const row = page.getByTestId('setting-numbering-ticket_prefix');
		await expect(row).toContainText('Locked by E2E MSP');
		await expect(row.getByRole('textbox', { name: 'Ticket prefix' })).toHaveValue('MSP');
		await expect(row.getByRole('textbox', { name: 'Ticket prefix' })).toBeDisabled();

		// Releasing it at the MSP brings back the default.
		await openOrg(page, 'E2E MSP');
		await page.getByRole('button', { name: 'Inherit Ticket prefix' }).click();
		await expect(page.getByTestId('setting-numbering-ticket_prefix')).toContainText('Default');
		await openOrg(page, client);
		const released = page.getByTestId('setting-numbering-ticket_prefix');
		await expect(released.getByRole('textbox', { name: 'Ticket prefix' })).toHaveValue('T');
		await expect(released.getByRole('textbox', { name: 'Ticket prefix' })).toBeEnabled();
	});
});
