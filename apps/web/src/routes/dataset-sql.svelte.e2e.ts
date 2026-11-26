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

test.describe('datasets SQL console', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('admins run read-only queries', async ({ browser }) => {
		const page = await signIn(browser);
		await page.goto('/datasets');
		await page.getByRole('link', { name: 'SQL console' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('SQL console');
		// Typing before the page hydrates is undone when it does.
		await page.locator('html[data-hydrated]').waitFor({ state: 'attached' });
		const query = page.getByLabel('SQL query');

		await query.fill("SELECT 41 + 1 AS answer, 'ok' AS status");
		await page.getByRole('button', { name: 'Run' }).click();
		const result = page.getByRole('table', { name: 'Query result' });
		await expect(result.getByRole('columnheader')).toHaveText(['answer', 'status']);
		await expect(result).toContainText('42');

		// Nothing outside the datasets schema; nothing written.
		await query.fill('SELECT * FROM public.users');
		await page.getByRole('button', { name: 'Run' }).click();
		await expect(page.getByRole('alert')).toContainText('permission denied');
		await query.fill("SELECT set_config('role', 'huxley_app', true)");
		await page.getByRole('button', { name: 'Run' }).click();
		await expect(page.getByRole('alert')).toContainText('permission denied');
	});
});
