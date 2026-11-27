import { expect, request, test, type APIResponse, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack (ADR-079): mileage and a parts receipt on a ticket, an
// internal expense on the Expenses page. Needs the service-desk preset.
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

async function ok<T>(response: Promise<APIResponse>) {
	const r = await response;
	expect(r.ok(), await r.text()).toBeTruthy();
	return (await r.json()) as T;
}

test.describe('expenses', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'tickets are off in this run');

	test('mileage and a receipt on a ticket, and an internal expense', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		await ok(api.put('/api/v1/modules/expenses', { data: { enabled: true } }));
		const orgs = await ok<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = await ok<{ id: string }>(
			api.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Spendco ${stamp}` }
			})
		);
		const ticket = await ok<{ id: string }>(
			api.post('/api/v1/tickets', { data: { orgId: client.id, title: `Router swap ${stamp}` } })
		);
		await api.dispose();

		await signIn(page);
		await page.goto(`/tickets/${ticket.id}`);
		const card = page
			.getByRole('region', { name: 'Expenses' })
			.or(page.locator('section', { has: page.getByRole('heading', { name: 'Expenses' }) }));
		// Mileage: 20 miles at the default 0.70.
		await card.getByRole('button', { name: 'Log' }).click();
		let dialog = page.getByRole('dialog', { name: 'Log an expense' });
		await dialog.getByLabel('Category').selectOption({ label: 'Mileage' });
		await dialog.getByLabel('What for').fill('To the client');
		await dialog.getByLabel('Distance').fill('20');
		await dialog.getByRole('button', { name: 'Log it' }).click();
		const list = page.getByRole('list', { name: 'Expenses' });
		await expect(list).toContainText('To the client');
		await expect(list).toContainText('$14.00');

		// Parts, with the receipt.
		await card.getByRole('button', { name: 'Log' }).click();
		dialog = page.getByRole('dialog', { name: 'Log an expense' });
		await dialog.getByLabel('Category').selectOption({ label: 'Parts and supplies' });
		await dialog.getByLabel('What for').fill('New router');
		await dialog.getByLabel('Amount (tax included)').fill('89.50');
		await dialog.getByLabel('Receipt').setInputFiles({
			name: 'receipt.png',
			mimeType: 'image/png',
			buffer: Buffer.from('89504e470d0a1a0a', 'hex')
		});
		await dialog.getByRole('button', { name: 'Log it' }).click();
		const router = list.getByRole('listitem').filter({ hasText: 'New router' });
		await expect(router).toContainText('$89.50');
		await expect(router.getByRole('link', { name: 'Receipt for New router' })).toBeVisible();
		await expect(router).not.toContainText('No receipt');

		// An internal expense on the Expenses page: flagged until there's a receipt.
		await openFromMenu(page, 'Expenses');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Expenses');
		await page.getByRole('button', { name: 'Log an expense' }).click();
		dialog = page.getByRole('dialog', { name: 'Log an expense' });
		await dialog.getByLabel('Category').selectOption({ label: 'Software and subscriptions' });
		await dialog.getByLabel('What for').fill(`Password manager ${stamp}`);
		await dialog.getByLabel('Amount (tax included)').fill('36');
		await dialog.getByLabel('For', { exact: true }).selectOption({ label: 'E2E MSP (internal)' });
		await dialog.getByRole('button', { name: 'Log it' }).click();
		const internal = page
			.getByRole('list', { name: 'Expenses' })
			.getByRole('listitem')
			.filter({ hasText: `Password manager ${stamp}` });
		await expect(internal).toContainText('No receipt');
		await internal.getByLabel(`Receipt for Password manager ${stamp}`).setInputFiles({
			name: 'invoice.pdf',
			mimeType: 'application/pdf',
			buffer: Buffer.from('%PDF-1.4\n%%EOF')
		});
		await expect(internal).not.toContainText('No receipt');
	});

	test('drafts are submitted as a report, which waits for approval', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		await ok(api.put('/api/v1/modules/expenses', { data: { enabled: true } }));
		const orgs = await ok<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		const categories = await ok<{ id: string; name: string }[]>(
			api.get('/api/v1/expense-categories')
		);
		const meals = categories.find((c) => c.name === 'Meals')!.id;
		await ok(
			api.post('/api/v1/expenses', {
				data: {
					categoryId: meals,
					spentOn: new Date().toISOString().slice(0, 10),
					description: `Team lunch ${stamp}`,
					amount: '42',
					orgId: root
				}
			})
		);
		await api.dispose();

		await signIn(page);
		await page.goto('/expenses');
		const submit = page.locator('form[action="?/submitReport"]');
		await expect(submit).toContainText(`Team lunch ${stamp}`);
		await submit.getByRole('textbox', { name: 'Title' }).fill(`Lunches ${stamp}`);
		await submit.getByRole('button', { name: 'Submit' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Lunches ${stamp}`);
		await expect(page.getByText('Waiting for approval', { exact: true })).toBeVisible();
		await expect(page.getByText('1 no receipt')).toBeVisible();
		// Submitted: no longer deletable.
		await expect(page.getByRole('button', { name: `Delete Team lunch ${stamp}` })).toBeHidden();
		await page.getByLabel('Add a comment').fill('Receipt to follow');
		await page.getByRole('button', { name: 'Comment' }).click();
		await expect(page.getByRole('list', { name: 'Comments' })).toContainText('Receipt to follow');
	});
});
