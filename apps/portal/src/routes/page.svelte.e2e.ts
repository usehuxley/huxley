import { expect, test } from '@playwright/test';

test('home page renders the portal shell', async ({ page }) => {
	await page.goto('/');
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help?');
	await expect(page.getByRole('navigation', { name: 'Main navigation' })).toBeVisible();
});

test('login page offers a magic link', async ({ page }) => {
	await page.goto('/login');
	await expect(page.getByLabel('Work email')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Email me a sign-in link' })).toBeVisible();
});

test('responses carry a content security policy', async ({ request }) => {
	const response = await request.get('/login');
	expect(response.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
});
