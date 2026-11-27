import { expect, request, test, type APIResponse, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the service-desk preset (tickets and time).
// The contracts and billing modules are turned on here.
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

test.describe('billing', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'tickets are off in this run');

	test('time is billed in a run, approved, sent and credited', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const today = new Date().toISOString().slice(0, 10);
		const weekAgo = new Date(Date.now() - 7 * 86_400_000).toISOString().slice(0, 10);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'billing']) {
			await ok(api.put(`/api/v1/modules/${module}`, { data: { enabled: true } }));
		}
		const orgs = await ok<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		await ok(
			api.put('/api/v1/billing/settings', {
				data: {
					orgId: root,
					hourlyRate: '150',
					currency: 'USD',
					paymentDays: 30,
					approvedTimeOnly: false
				}
			})
		);
		const client = await ok<{ id: string }>(
			api.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Billed ${stamp}` }
			})
		);
		const ticket = await ok<{ id: string }>(
			api.post('/api/v1/tickets', { data: { orgId: client.id, title: `Server down ${stamp}` } })
		);
		const types = await ok<{ id: string; name: string }[]>(api.get('/api/v1/work-types'));
		await ok(
			api.post(`/api/v1/tickets/${ticket.id}/time`, {
				data: {
					workTypeId: types.find((t) => t.name === 'Remote support')!.id,
					workedOn: today,
					minutes: 120
				}
			})
		);
		await api.dispose();

		await signIn(page);
		await openFromMenu(page, 'Billing');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Billing');
		await page.getByLabel('From', { exact: true }).fill(weekAgo);
		await page.getByLabel('To', { exact: true }).fill(today);
		await page.getByRole('button', { name: 'Show' }).click();
		const unbilled = page.getByRole('table', { name: 'Unbilled' });
		const row = unbilled.getByRole('row').filter({ hasText: `Billed ${stamp}` });
		await expect(row).toContainText('$300.00');
		await row.getByRole('checkbox').check();
		await page.getByRole('button', { name: 'Bill 1 client' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Invoices');
		await page.getByRole('table', { name: 'Invoices' }).getByRole('link').first().click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Billed ${stamp}`);
		await expect(page.getByRole('region', { name: 'Lines' })).toContainText('$300.00');

		await page.getByRole('button', { name: 'Approve' }).click();
		await expect(page.getByText('Approved', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Mark sent' }).click();
		await expect(page.getByText('Sent', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Credit memo' }).click();
		const credit = page.getByRole('dialog', { name: 'Credit memo' });
		await credit.getByLabel('Why').fill('Goodwill');
		await credit.getByRole('button', { name: 'Make the credit memo' }).click();
		await expect(page.getByText('Credit memo', { exact: true }).first()).toBeVisible();
		await expect(page.getByRole('region', { name: 'Lines' })).toContainText('-$300.00');
	});

	test('an invoice changed by hand goes to review, and the approved review approves it', async ({
		page
	}) => {
		test.setTimeout(150_000);
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const today = new Date().toISOString().slice(0, 10);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'billing']) {
			await ok(api.put(`/api/v1/modules/${module}`, { data: { enabled: true } }));
		}
		const me = (await ok<{ id: string }>(api.get('/api/v1/auth/me'))).id;
		const orgs = await ok<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		const name = `Reviewed ${stamp}`;
		const client = await ok<{ id: string }>(
			api.post('/api/v1/orgs', { data: { parentId: root, kind: 'client', name } })
		);
		// Rules are instance-wide: this one names the owner, for this client only.
		const rule = await ok<{ id: string }>(
			api.post('/api/v1/billing/review-rules', {
				data: {
					name: `Review ${stamp}`,
					kind: 'expression',
					expression: `invoice.orgName == '${name}'`,
					reviewers: `user:${me}`
				}
			})
		);
		const pack = await ok<{ uploadId: string; version: string }>(
			api.post(`/api/v1/packs/builtin/huxley/invoice-review?orgId=${client.id}`)
		);
		const installed = await ok<{ items: { workflows: Record<string, string> } }>(
			api.post(`/api/v1/packs/uploads/${pack.uploadId}/install`, {
				data: { orgId: client.id, version: pack.version }
			})
		);
		for (const id of Object.values(installed.items.workflows)) {
			await ok(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }));
		}
		const ticket = await ok<{ id: string }>(
			api.post('/api/v1/tickets', { data: { orgId: client.id, title: `Backups ${stamp}` } })
		);
		const types = await ok<{ id: string; name: string }[]>(api.get('/api/v1/work-types'));
		await ok(
			api.post(`/api/v1/tickets/${ticket.id}/time`, {
				data: {
					workTypeId: types.find((t) => t.name === 'Remote support')!.id,
					workedOn: today,
					minutes: 60
				}
			})
		);
		const run = await ok<{ id: string }>(
			api.post('/api/v1/billing/runs', {
				data: { orgId: root, from: today, to: today, clients: [client.id] }
			})
		);
		const [invoice] = await ok<{ id: string }[]>(api.get(`/api/v1/invoices?runId=${run.id}`));

		try {
			await signIn(page);
			await page.goto(`/billing/invoices/${invoice.id}`);
			await expect(page.getByRole('list', { name: 'Why it needs review' })).toContainText(
				`Review ${stamp}`
			);
			// A price changed by hand: the out-of-the-box rule matches too.
			await page.getByRole('region', { name: 'Lines' }).getByLabel('Price').fill('99');
			await page.getByRole('button', { name: 'Save' }).click();
			await expect(page.getByRole('list', { name: 'Why it needs review' })).toContainText(
				'1 line changed by hand'
			);
			await expect(page.getByRole('button', { name: 'Approve', exact: true })).toBeHidden();

			await page.getByLabel('Add a comment').fill('Discounted as agreed');
			await page.getByRole('button', { name: 'Comment', exact: true }).click();
			await expect(page.getByRole('list', { name: 'Open comments' })).toContainText(
				'Discounted as agreed'
			);

			await page.getByRole('button', { name: 'Submit for review' }).click();
			await expect(page.getByText('In review', { exact: true }).first()).toBeVisible();
			await expect(async () => {
				await page.reload();
				await expect(page.getByText('Your review')).toBeVisible({ timeout: 1000 });
			}).toPass({ timeout: 60_000 });
			await page.getByRole('button', { name: 'Approve', exact: true }).click();
			await expect(async () => {
				await page.reload();
				await expect(page.getByRole('button', { name: 'Mark sent' })).toBeVisible({
					timeout: 1000
				});
			}).toPass({ timeout: 60_000 });
			await expect(page.getByRole('list', { name: 'Review history' })).toContainText('Approved');
		} finally {
			await api.delete(`/api/v1/billing/review-rules/${rule.id}`);
			await api.dispose();
		}
	});
});
