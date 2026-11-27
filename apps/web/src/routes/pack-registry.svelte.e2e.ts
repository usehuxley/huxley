import { expect, test, type Page } from '@playwright/test';

// Full-stack: needs the API configured with a pack registry serving
// e2e-fixtures/registry-pack (the CI e2e job builds, indexes and serves it,
// and sets E2E_PACK_REGISTRY).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

test.describe('pack registries', () => {
	test.skip(
		!email || !password || !process.env.E2E_PACK_REGISTRY,
		'set E2E_PACK_REGISTRY with the API serving a pack registry'
	);

	test('a pack from a registry is reviewed and installed', async ({ page }) => {
		await signIn(page);
		await page.goto('/settings/packs');
		const packs = page.getByRole('list', { name: 'E2E packs packs' });
		const row = packs.getByRole('listitem').filter({ hasText: 'e2e/hello' });
		await expect(row).toContainText('Hello pack');
		await expect(row).toContainText('Signed by an unknown key');

		await row.getByRole('button', { name: 'Review Hello pack' }).click();
		const review = page.getByTestId('pack-review');
		await expect(review).toContainText('registry-hello');
		await expect(review).toContainText('Adds');
		await page.getByRole('button', { name: 'Install' }).click();
		await expect(page.getByText('Installed e2e/hello 1.0.0.')).toBeVisible();
		await expect(row).toContainText('Installed');
	});
});
