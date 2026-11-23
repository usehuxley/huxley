import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** Calls the API with only a bearer token, as a script would. */
async function getWithToken(path: string, token: string): Promise<number> {
	const api = await request.newContext({ baseURL: apiUrl });
	const res = await api.get(path, { headers: { authorization: `Bearer ${token}` } });
	await api.dispose();
	return res.status();
}

test.describe('API tokens', () => {
	test.skip(!email || !password || !apiUrl, 'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL');

	test('a personal token works from a script until revoked', async ({ page }) => {
		await signIn(page);
		await page.goto('/profile');
		const name = `script ${Date.now().toString(36)}`;
		await page.getByLabel('Token name').fill(name);
		await page.getByRole('button', { name: 'Create token' }).click();
		const secret = (await page.getByTestId('token-secret').innerText()).trim();
		expect(secret).toMatch(/^hxp_/);
		expect(await getWithToken('/api/v1/orgs', secret)).toBe(200);

		await page.getByRole('button', { name: `Revoke ${name}` }).click();
		await expect(page.getByRole('button', { name: `Revoke ${name}` })).toHaveCount(0);
		expect(await getWithToken('/api/v1/orgs', secret)).toBe(401);
	});

	test('an owner creates a service account with a non-expiring token', async ({ page }) => {
		await signIn(page);
		await page.goto('/settings/service-accounts');
		const name = `Sync ${Date.now().toString(36)}`;
		await page.getByLabel('Name').fill(name);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { name })).toBeVisible();

		await page.getByLabel('Token name').fill('prod');
		await page.getByLabel('Expires').selectOption('never');
		await page.getByRole('button', { name: 'Create token' }).click();
		const secret = (await page.getByTestId('token-secret').innerText()).trim();
		await expect(page.getByText('Expires never')).toBeVisible();
		// No roles yet: it can authenticate but not read users.
		expect(await getWithToken('/api/v1/orgs', secret)).toBe(200);
		expect(await getWithToken('/api/v1/users', secret)).toBe(403);

		// Disabling the account stops its tokens.
		await page.getByRole('button', { name: 'Disable' }).click();
		await expect(page.getByText('disabled', { exact: true })).toBeVisible();
		expect(await getWithToken('/api/v1/orgs', secret)).toBe(401);
	});
});
