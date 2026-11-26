import { expect, test } from '@playwright/test';

test('signed-out visitors are sent to sign in', async ({ page }) => {
	await page.goto('/');
	await expect(page).toHaveURL(/\/login$/);
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(/^Sign in to /);
});

test('sign-in offers a link or a password', async ({ page }) => {
	await page.goto('/login');
	await expect(page.getByLabel('Work email')).toBeVisible();
	await expect(page.getByRole('button', { name: 'Email me a sign-in link' })).toBeVisible();
	await page.getByRole('button', { name: 'Sign in with a password' }).click();
	await expect(page.getByLabel('Password')).toBeVisible();
});

test('opening a sign-in link asks before signing in', async ({ page }) => {
	await page.goto('/auth/link?token=hxpl_example');
	await expect(page.getByRole('button', { name: 'Sign in' })).toBeVisible();
});

test('responses carry a content security policy', async ({ request }) => {
	const response = await request.get('/login');
	expect(response.headers()['content-security-policy']).toContain("frame-ancestors 'none'");
});
