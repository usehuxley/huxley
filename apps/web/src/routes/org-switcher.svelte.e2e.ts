import { expect, test } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

test.describe('organization switcher', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('find an organization, switch to it, and switch back to all', async ({ page }) => {
		const name = `Switchco ${Date.now().toString(36)}`;

		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		await page.goto('/settings/organizations');
		await page.getByRole('button', { name: 'Add' }).first().click();
		await page.getByLabel('Name').fill(name);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByText(name)).toBeVisible();

		const switcher = page.getByTestId('org-switcher');
		await expect(switcher).toContainText('All organizations');
		await switcher.click();
		const search = page.getByRole('combobox', { name: 'Switch organization' });
		await search.fill(name.slice(0, 12));
		await expect(page.getByRole('option', { name: new RegExp(name) })).toBeVisible();
		await search.press('Enter');

		await expect(page.getByRole('heading', { name })).toBeVisible();
		await expect(switcher).toContainText(name);
		await page.reload();
		await expect(page.getByTestId('org-switcher')).toContainText(name);

		await page.getByTestId('org-switcher').click();
		await page.getByRole('option', { name: /All organizations/ }).click();
		await expect(page.getByTestId('org-switcher')).toContainText('All organizations');
	});
});
