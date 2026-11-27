import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

test.describe('attachments', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('upload a file to an organization, download it, and delete it', async ({ page }) => {
		const name = `runbook-${Date.now().toString(36)}.txt`;
		const content = 'Restart the print spooler, then call Dana.';

		await page.goto('/login');
		await page.getByLabel('Email').fill(email!);
		await page.getByLabel('Password').fill(password!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		await page.goto('/settings/organizations');
		await page.getByRole('link', { name: 'E2E MSP' }).click();
		await page.getByLabel('File').setInputFiles({
			name,
			mimeType: 'text/plain',
			buffer: Buffer.from(content)
		});
		await page.getByRole('button', { name: 'Upload' }).click();
		await expect(page.getByRole('link', { name })).toBeVisible();

		const downloading = page.waitForEvent('download');
		await page.getByRole('link', { name }).click();
		const download = await downloading;
		expect(download.suggestedFilename()).toBe(name);
		expect(await readFile(await download.path())).toEqual(Buffer.from(content));

		await page.getByRole('button', { name: `Delete ${name}` }).click();
		await expect(page.getByRole('link', { name })).toHaveCount(0);
	});
});
