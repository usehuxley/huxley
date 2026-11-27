import { readFile } from 'node:fs/promises';
import { expect, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts).
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

test.describe('settings (as the bootstrap owner)', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	test('owner creates an org and a user, assigns a role, and sees it audited', async ({ page }) => {
		const suffix = Date.now().toString(36);
		const orgName = `Contoso ${suffix}`;
		const userEmail = `tech-${suffix}@example.com`;

		await signIn(page);
		await page
			.getByRole('navigation', { name: 'Spaces' })
			.getByRole('link', { name: 'Settings' })
			.click();
		await expect(page).toHaveURL(/\/settings\/organizations$/);

		// Create a client under the root organization (the first row).
		await page.getByRole('button', { name: 'Add' }).first().click();
		await page.getByLabel('Name').fill(orgName);
		await page.getByRole('button', { name: 'Create' }).click();
		await expect(page.getByText(orgName)).toBeVisible();

		// Create a user with a generated password.
		await page.getByRole('link', { name: 'Users' }).click();
		await page.getByRole('button', { name: 'Add user' }).click();
		await page.getByLabel('Name').fill('Taylor Tech');
		await page.locator('#new-email').fill(userEmail);
		await page.getByRole('button', { name: 'Create user' }).click();
		await expect(page.getByText(`Created ${userEmail}.`)).toBeVisible();
		await expect(page.getByText('Temporary password, shown only once')).toBeVisible();

		// Give them the Technician role at the new org.
		await page.getByRole('link', { name: 'assign a role' }).click();
		await expect(page.getByRole('heading', { name: 'Taylor Tech' })).toBeVisible();
		await page.getByLabel('Role', { exact: true }).selectOption({ label: 'Technician' });
		await page.getByLabel('Organization', { exact: true }).selectOption({ label: `— ${orgName}` });
		await page.getByRole('button', { name: 'Assign' }).click();
		await expect(page.getByText(`at ${orgName} and its sub-organizations`)).toBeVisible();

		// The audit log records it.
		await page.getByRole('link', { name: 'Audit log' }).click();
		await page.getByLabel('Filter by action prefix').fill('role.assigned');
		await page.getByRole('button', { name: 'Filter' }).click();
		await expect(page).toHaveURL(/action=role\.assigned/);
		await expect(page.getByText('role: technician').first()).toBeVisible();

		// …and exports it, filtered, as CSV.
		await page.getByLabel('Export format').selectOption('csv');
		const downloading = page.waitForEvent('download');
		await page.getByRole('button', { name: 'Export' }).click();
		const download = await downloading;
		expect(download.suggestedFilename()).toMatch(/^huxley-audit-\d+\.csv$/);
		const csv = await readFile(await download.path(), 'utf8');
		const [header, ...rows] = csv.trim().split('\n');
		expect(header).toMatch(/^seq,id,occurred_at,/);
		expect(rows.length).toBeGreaterThan(0);
		expect(rows.filter((r) => !r.includes(',role.assigned,'))).toEqual([]);
	});

	test('owner turns a module on with its dependencies, then off', async ({ page }) => {
		// Starts from an install with every module off, and changes the
		// instance's modules: only in that run of the suite.
		test.skip(
			(process.env.E2E_PRESET ?? 'automation') !== 'automation',
			'modules are on in this run'
		);
		await signIn(page);
		await page.goto('/settings/modules');
		await expect(page.getByText('No modules on: automation engine only.')).toBeVisible();

		const row = page.getByRole('listitem').filter({ hasText: 'Sales and quoting' });
		await expect(row.getByText('also turns on Clients and contacts, Contracts')).toBeVisible();
		await row.getByRole('button', { name: 'Turn on' }).click();
		await expect(
			page.getByText('Turned on: Clients and contacts, Contracts, Sales and quoting')
		).toBeVisible();
		await expect(row.getByText('On: arrives in Phase 10')).toBeVisible();

		// Contracts can't be turned off while Sales and quoting needs it.
		const contracts = page.getByRole('listitem').filter({ hasText: 'Agreements (coverage' });
		await contracts.getByRole('button', { name: 'Turn off' }).click();
		await expect(page.getByText(/other enabled modules depend on it/i)).toBeVisible();

		await row.getByRole('button', { name: 'Turn off' }).click();
		await expect(page.getByText('Turned off: Sales and quoting')).toBeVisible();

		// Leave the instance as it was: every module off.
		await contracts.getByRole('button', { name: 'Turn off' }).click();
		await expect(page.getByText('Turned off: Contracts')).toBeVisible();
		const crm = page.getByRole('listitem').filter({ hasText: 'Companies, contacts, sites' });
		await crm.getByRole('button', { name: 'Turn off' }).click();
		await expect(page.getByText('Turned off: Clients and contacts')).toBeVisible();
		await expect(page.getByText('No modules on: automation engine only.')).toBeVisible();
	});
});
