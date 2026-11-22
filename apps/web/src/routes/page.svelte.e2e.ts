import { expect, test } from '@playwright/test';

// Full-stack tests: they need the Huxley API running with a bootstrapped user.
// CI provides one (see the e2e job). Locally: `just dev-up`, `just server`,
// bootstrap a user, then set E2E_EMAIL and E2E_PASSWORD.
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;
const fullStack = Boolean(email && password);

test('healthz responds ok', async ({ request }) => {
	const response = await request.get('/healthz');
	expect(response.ok()).toBe(true);
	expect(await response.json()).toEqual({ status: 'ok' });
});

test('responses carry a content security policy', async ({ request }) => {
	const response = await request.get('/login');
	expect(response.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
	expect(response.headers()['x-content-type-options']).toBe('nosniff');
});

test('login page renders the sign-in form', async ({ page }) => {
	await page.goto('/login');
	await expect(page.getByRole('heading', { name: 'Sign in to Huxley' })).toBeVisible();
	await expect(page.getByLabel('Email')).toBeVisible();
	await expect(page.getByLabel('Password')).toBeVisible();
});

test.describe('with the API', () => {
	test.skip(!fullStack, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('unauthenticated visitors are sent to sign in, then back', async ({ page }) => {
		await page.goto('/profile');
		await expect(page).toHaveURL(/\/login\?next=%2Fprofile$/);

		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();

		await expect(page).toHaveURL(/\/profile$/);
		await expect(page.getByRole('heading', { name: 'Your profile' })).toBeVisible();
		await expect(page.getByText('This device')).toBeVisible();
	});

	test('a wrong password shows an error', async ({ page }) => {
		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill('definitely not the password');
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('alert')).toHaveText('Invalid email or password.');
	});

	test('signing in shows the app and signing out ends the session', async ({ page }) => {
		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();

		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
		await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
		await expect(page.getByText('Operational')).toBeVisible();

		await page.getByRole('button', { name: 'Sign out' }).click();
		await expect(page).toHaveURL(/\/login$/);

		// The old session no longer grants access.
		await page.goto('/');
		await expect(page).toHaveURL(/\/login/);
	});
});
