import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the service-desk preset (crm, tickets). The
// contracts module is optional: the test turns it on.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** A client with a ticket and three entries today: its and the ticket's ids. */
async function setUp(stamp: string) {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const on = await api.put('/api/v1/modules/contracts', { data: { enabled: true } });
	expect(on.status(), await on.text()).toBe(200);
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `Covered ${stamp}`
		}
	});
	expect(client.status(), await client.text()).toBe(201);
	const { id: clientId } = (await client.json()) as { id: string };
	const ticket = await api.post('/api/v1/tickets', {
		data: { orgId: clientId, title: `Backups ${stamp}` }
	});
	expect(ticket.status(), await ticket.text()).toBe(201);
	const { id: ticketId } = (await ticket.json()) as { id: string };
	const types = (await (await api.get('/api/v1/work-types')).json()) as {
		id: string;
		name: string;
	}[];
	const type = (name: string) => types.find((t) => t.name === name)!.id;
	const today = new Date().toISOString().slice(0, 10);
	for (const [name, minutes] of [
		['Remote support', 45],
		['Remote support', 30],
		['Onsite', 20]
	] as const) {
		const logged = await api.post(`/api/v1/tickets/${ticketId}/time`, {
			data: { workTypeId: type(name), workedOn: today, minutes, billable: true }
		});
		expect(logged.status(), await logged.text()).toBe(201);
	}
	await api.dispose();
	return { clientId, ticketId };
}

test.describe('agreements', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm and servicedesk modules are off in this run');

	test('an agreement covers remote support up to an hour, and the rest is billable', async ({
		page
	}) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const { clientId, ticketId } = await setUp(stamp);
		await signIn(page);
		await page.goto(`/clients/${clientId}?tab=agreements`);
		await expect(async () => {
			await page.getByRole('button', { name: 'New agreement' }).click();
			await expect(page.getByRole('dialog')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });

		const dialog = page.getByRole('dialog');
		const firstOfMonth = `${new Date().toISOString().slice(0, 8)}01`;
		await dialog.locator('#ag-name').fill(`Managed ${stamp}`);
		await dialog.locator('#ag-kind').selectOption('managed');
		await dialog.locator('#ag-status').selectOption('active');
		await dialog.locator('#ag-starts').fill(firstOfMonth);
		await dialog.getByLabel('All work types').uncheck();
		await dialog
			.getByRole('group', { name: 'Covered work types' })
			.getByLabel('Remote support', { exact: true })
			.check();
		await dialog.locator('#ag-allowance').fill('1');
		await dialog.locator('#ag-rate').fill('100');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Agreement added.')).toBeVisible();

		// This month: 45 + 15 covered; 15 + 20 billable at 100/h.
		const usage = page.getByTestId('agreement-usage');
		await expect(usage).toContainText('1 h covered, 0.58 h billable (58.33 USD)');

		// The ticket shows each entry's outcome.
		await page.goto(`/tickets/${ticketId}`);
		const entries = page.getByRole('list', { name: 'Time entries' });
		await expect(entries).toContainText('Covered');
		await expect(entries).toContainText('Partly covered');
		await expect(entries).toContainText('Billable');
		await expect(entries).toContainText(`Managed ${stamp}`);
	});
});
