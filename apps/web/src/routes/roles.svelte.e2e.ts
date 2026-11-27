import { expect, test } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

test.describe('custom roles', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('create a custom role and see why the owner can do things', async ({ page }) => {
		const name = `Dispatcher ${Date.now().toString(36)}`;

		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		await page.goto('/settings/roles');
		await page.getByRole('button', { name: 'New role' }).click();
		await page.locator('#roleName').fill(name);
		await page.getByLabel('View organizations').check();
		await page.getByLabel('View the audit log').check();
		await page.getByRole('button', { name: 'Create role' }).click();
		await expect(page.getByRole('heading', { name })).toBeVisible();

		await page.locator('#roleDescription').fill('Routes tickets');
		await page.getByLabel('View the audit log').uncheck();
		await page.getByRole('button', { name: 'Save role' }).click();
		await expect(page.getByText('Saved.', { exact: true })).toBeVisible();

		page.once('dialog', (d) => void d.accept());
		await page.getByRole('button', { name: 'Delete role' }).click();
		await expect(page).toHaveURL(/\/settings\/roles$/);
		await expect(page.getByRole('link', { name })).toHaveCount(0);

		// The explainer on the owner's own user page.
		await page.goto('/settings/users');
		await page.getByRole('table').getByRole('link', { name: 'E2E User' }).click();
		const audit = page.getByTestId('perm-audit:read');
		await expect(audit).toContainText('Allowed');
		await expect(audit).toContainText('Owner at E2E MSP');
	});
});
