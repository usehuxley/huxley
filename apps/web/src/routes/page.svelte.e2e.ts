import { expect, test } from '@playwright/test';

test('home page renders the app shell', async ({ page }) => {
	await page.goto('/');
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
	// No API runs during e2e, so the status card reports it as unreachable.
	await expect(page.getByText('API unreachable')).toBeVisible();
});

test('login page renders the sign-in form', async ({ page }) => {
	await page.goto('/login');
	await expect(page.getByRole('heading', { name: 'Sign in to Huxley' })).toBeVisible();
	await expect(page.getByLabel('Email')).toBeVisible();
	await expect(page.getByLabel('Password')).toBeVisible();
});

test('responses carry a content security policy', async ({ request }) => {
	const response = await request.get('/login');
	expect(response.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
	expect(response.headers()['x-content-type-options']).toBe('nosniff');
});

test('healthz responds ok', async ({ request }) => {
	const response = await request.get('/healthz');
	expect(response.ok()).toBe(true);
	expect(await response.json()).toEqual({ status: 'ok' });
});
