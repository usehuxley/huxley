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

test.describe('AI settings', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('a provider is set, tested, logged and removed', async ({ browser }) => {
		const page = await signIn(browser);
		await page.goto('/settings/ai');
		await page.getByLabel('Provider', { exact: true }).selectOption('openai_compatible');
		await page.getByLabel('Model').fill('local-model');
		// Nothing answers at a .invalid host: the test fails without reaching any real service.
		await page.getByLabel('Base URL').fill('https://ai.example.invalid/v1');
		await page.getByLabel('API key').fill('sk-e2e-not-real');
		await page.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('AI provider saved.')).toBeVisible();
		await expect(page.getByTestId('ai-effective')).toContainText('OpenAI-compatible');
		await expect(page.getByTestId('ai-effective')).toContainText('local-model');
		// The key is never shown again.
		await expect(page.getByLabel('API key')).toHaveValue('');
		await expect(page.locator('body')).not.toContainText('sk-e2e-not-real');

		await page.getByRole('button', { name: 'Test' }).click();
		await expect(page.getByTestId('ai-test')).toContainText("couldn't reach the provider");
		await page.reload();
		await expect(page.getByRole('list', { name: 'AI calls' })).toContainText('provider_test');
		await expect(page.getByRole('list', { name: 'AI calls' })).toContainText('failed');

		await page.getByRole('button', { name: 'Remove' }).click();
		await expect(page.getByText('AI provider removed.')).toBeVisible();
		await expect(page.getByText('No AI provider')).toBeVisible();
	});
});
