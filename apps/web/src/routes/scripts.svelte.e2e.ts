import { expect, test, type Browser, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;

/** Waits until the page is interactive: typing before it hydrates is undone
 * when it does. */
async function hydrated(page: Page) {
	await page.locator('html[data-hydrated]').waitFor({ state: 'attached' });
}

async function signIn(browser: Browser): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	// Interactive before following links (or they're full page loads).
	await hydrated(page);
	return page;
}

test.describe('scripts', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('every saved change is a version', async ({ browser }) => {
		const page = await signIn(browser);
		const name = `Greeter ${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		await openFromMenu(page, 'Scripts');
		await hydrated(page);
		await page.getByLabel('Name').fill(name);
		await page.getByLabel('Language').selectOption('python');
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);
		await hydrated(page);
		// The parameters show as the form a step will ask.
		await expect(page.getByLabel('Name *')).toBeVisible();

		await page.getByLabel('Script source').fill('print("version two")\n');
		await page.getByLabel('What changed').fill('simpler');
		await page.getByRole('button', { name: 'Save as version 2' }).click();
		await expect(page.getByText('Saved as version 2.')).toBeVisible();

		const versions = page.getByRole('list', { name: 'Versions' });
		await expect(versions).toContainText('simpler');
		await versions.getByRole('link', { name: 'Version 1' }).click();
		await expect(page.getByLabel('Version 1 source')).toContainText('json.load');
		await expect(page.getByLabel('Script source')).toContainText('print("version two")');
	});

	test('the code editor highlights PowerShell and completes parameters', async ({ browser }) => {
		const page = await signIn(browser);
		// Nothing the page does breaks its Content Security Policy (the
		// editor's styles carry the page's nonce).
		const blocked: string[] = [];
		page.on('console', (m) => {
			if (m.text().includes('Content Security Policy')) blocked.push(m.text());
		});
		await openFromMenu(page, 'Scripts');
		await hydrated(page);
		await page
			.getByLabel('Name')
			.fill(`Pinger ${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`);
		await page.getByLabel('Language').selectOption('pwsh');
		await page.getByRole('button', { name: 'Create' }).click();
		const source = page.locator('[data-language="powershell"] .cm-content');
		// The editor and the PowerShell mode load on demand.
		await expect(source).toBeVisible({ timeout: 15_000 });
		// Highlighted: keywords and variables get their own spans.
		await expect(source.locator('span').first()).toBeVisible();
		// The new script's parameter, offered while typing.
		await source.click();
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.press('Enter');
		await page.keyboard.type('$Na');
		const options = page.getByRole('listbox');
		const option = options.getByRole('option', { name: /\$name/i });
		await expect(option).toBeVisible();
		// Clicked: Enter right as the list opens is ignored (CodeMirror waits a
		// moment, so a fast typist's Enter isn't swallowed).
		await option.click();
		// "$Na" became the parameter, on the line after the greeting.
		await expect(source).toContainText('"$name');
		// Broken parameters JSON is flagged as you type.
		const params = page.locator('#parameters .cm-content');
		await params.click();
		await page.keyboard.press('ControlOrMeta+End');
		await page.keyboard.type(',');
		await expect(page.locator('#parameters .cm-lintRange-error')).toHaveCount(1);
		expect(blocked).toEqual([]);
	});

	test('an admin enrols a script runner', async ({ browser }) => {
		const page = await signIn(browser);
		await page.goto('/settings/runners');
		const name = `runner-${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		await page.getByLabel('Runner name').fill(name);
		await page.getByRole('button', { name: 'Create token' }).click();
		await expect(page.getByTestId('runner-token')).toContainText('hxr_');
		await expect(page.getByText('huxley runner enroll --server')).toBeVisible();
	});
});
