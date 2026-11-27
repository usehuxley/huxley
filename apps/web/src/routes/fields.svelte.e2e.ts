import { expect, test } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

test.describe('custom fields and tags', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('an owner defines a field and a tag, then fills them in on an organization', async ({
		page
	}) => {
		const suffix = Date.now().toString(36);
		const fieldLabel = `Support tier ${suffix}`;
		const tagName = `Priority ${suffix}`;

		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		// Define a choice field and a tag.
		await page.goto('/settings/fields');
		await page.getByLabel('Field label').fill(fieldLabel);
		await page.getByLabel('Type').selectOption('select');
		await page.getByLabel('Options').fill('Gold\nSilver');
		await page.getByRole('button', { name: 'Add field' }).click();
		await expect(page.getByText(fieldLabel)).toBeVisible();
		await page.getByLabel('Tag name').fill(tagName);
		await page.getByLabel('Color').selectOption('warning');
		await page.getByRole('button', { name: 'Add tag' }).click();
		await expect(page.getByText(tagName)).toBeVisible();

		// Fill them in on the root organization.
		await page.goto('/settings/organizations');
		await page.getByRole('link', { name: 'E2E MSP' }).click();
		await page.getByLabel(fieldLabel).selectOption('Gold');
		await page.getByRole('button', { name: 'Save fields' }).click();
		await expect(page.getByText('Saved.')).toBeVisible();
		await page.getByLabel('Add tag').selectOption({ label: tagName });
		await page.getByRole('button', { name: 'Add', exact: true }).click();
		await expect(page.getByRole('button', { name: `Remove tag ${tagName}` })).toBeVisible();

		await page.reload();
		await expect(page.getByLabel(fieldLabel)).toHaveValue('Gold');
		await expect(page.getByText(tagName)).toBeVisible();

		// Archive the field again so other runs start clean.
		await page.goto('/settings/fields');
		await page.getByRole('button', { name: `Archive ${fieldLabel}` }).click();
		await expect(page.getByRole('button', { name: `Restore ${fieldLabel}` })).toBeVisible();
	});
});
