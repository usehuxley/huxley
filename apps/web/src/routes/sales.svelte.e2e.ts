import { expect, request, test, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the service-desk preset (crm). The contracts
// and sales modules are optional: the test turns them on.
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

test.describe('sales pipeline', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm module is off in this run');

	test('a lead is converted, moved through the pipeline and won', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const company = `Initech ${stamp}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'sales']) {
			const on = await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
			expect(on.status(), await on.text()).toBe(200);
		}
		await api.dispose();

		await signIn(page);
		await openFromMenu(page, 'Leads');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Leads');
		await page.getByRole('button', { name: 'New lead' }).click();
		const add = page.getByRole('dialog', { name: 'New lead' });
		await add.getByLabel('Company').fill(company);
		await add.getByLabel('First name').fill('Peter');
		await add.getByLabel('Last name').fill('Gibbons');
		await add.getByLabel('Email').fill(`peter-${stamp}@initech.example`);
		await add.getByLabel('Source').fill('Referral');
		await add.getByRole('button', { name: 'Add lead' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(company);

		// Converting makes the client, the contact and the opportunity.
		await page.getByRole('button', { name: 'Convert' }).click();
		const convert = page.getByRole('dialog', { name: `Convert ${company}` });
		await convert.getByLabel('Name', { exact: true }).fill(`Managed services ${stamp}`);
		await convert.getByLabel('Monthly recurring').fill('2400');
		await convert.getByLabel('One-time value').fill('5000');
		await convert.getByRole('button', { name: 'Convert' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Managed services ${stamp}`);
		await expect(page.getByLabel('Contact')).toHaveText(/Peter Gibbons/);

		// Moving along the stages keeps history.
		const stages = page.getByRole('group', { name: 'Stage' });
		await stages.getByRole('button').nth(1).click();
		await expect(
			page.getByRole('list', { name: 'Stage history' }).getByRole('listitem')
		).toHaveCount(2);

		// Won, with a reason.
		await page.getByRole('button', { name: 'Won', exact: true }).click();
		const won = page.getByRole('dialog', { name: 'Mark it won' });
		await won.getByLabel('Reason').selectOption({ label: 'Relationship' });
		await won.getByLabel('Note').fill('They liked our response times');
		await won.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('They liked our response times')).toBeVisible();
		await expect(page.getByText('100% likely')).toBeVisible();

		// It's on the list of won deals, with its client.
		await openFromMenu(page, 'Pipeline');
		await page.getByRole('button', { name: 'List' }).click();
		await page.getByLabel('Status').selectOption('won');
		const row = page
			.getByRole('table', { name: 'Opportunities' })
			.getByRole('row')
			.filter({ hasText: `Managed services ${stamp}` });
		await expect(row).toContainText(company);
		await expect(row).toContainText('$5,000');
	});
});
