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

test.describe('scripts', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('every saved change is a version', async ({ browser }) => {
		const page = await signIn(browser);
		const name = `Greeter ${Date.now().toString(36)}`;
		await page.getByRole('link', { name: 'Scripts' }).click();
		await page.getByLabel('Name').fill(name);
		await page.getByLabel('Language').selectOption('python');
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);
		// The parameters show as the form a step will ask.
		await expect(page.getByLabel('Name *')).toBeVisible();

		await page.getByLabel('Script source').fill('print("version two")\n');
		await page.getByLabel('What changed').fill('simpler');
		await page.getByRole('button', { name: 'Save as version 2' }).click();
		await expect(page.getByText('Saved as version 2.')).toBeVisible();

		const versions = page.getByRole('list', { name: 'Versions' });
		await expect(versions).toContainText('simpler');
		await versions.getByRole('link', { name: 'Version 1' }).click();
		await expect(page.getByLabel('Version 1 source')).toContainText('json.load');
		await expect(page.getByLabel('Script source')).toHaveValue('print("version two")\n');
	});
});
