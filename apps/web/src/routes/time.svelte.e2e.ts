import { expect, test, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the servicedesk module on (the e2e job's
// service-desk preset).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

test.describe('my time', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('non-ticket time is logged for the week, with a timer', async ({ page }) => {
		const note = `Intune course ${Date.now().toString(36)}`;
		await signIn(page);
		await openFromMenu(page, 'My time');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('My time');

		// The timer fills the time spent (at least a minute).
		await expect(async () => {
			await page.getByRole('button', { name: 'Start timer' }).click();
			await expect(page.getByRole('button', { name: 'Stop the timer' })).toBeVisible({
				timeout: 1000
			});
		}).toPass({ timeout: 15_000 });
		await page.getByRole('button', { name: 'Stop the timer' }).click();
		await expect(page.getByLabel('Time spent')).toHaveValue('1m');

		await page.getByLabel('Time spent').fill('2h');
		await page.getByLabel('Work type').selectOption({ label: 'Training' });
		await page.getByLabel('Notes').fill(note);
		await page.getByRole('button', { name: 'Log time' }).click();
		await expect(page.getByText('Time logged.')).toBeVisible();
		const week = page.getByRole('region', { name: 'This week' });
		await expect(week).toContainText(note);
		await expect(week).toContainText('2h · Training');
		await expect(page.getByText(/this week/)).not.toContainText('Nothing');

		await week.getByRole('button', { name: 'Delete 2h Training' }).first().click();
		await expect(page.getByText('Time deleted.')).toBeVisible();
		await expect(week).not.toContainText(note);
	});
});
