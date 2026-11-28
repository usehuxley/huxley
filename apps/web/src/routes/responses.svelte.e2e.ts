import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the servicedesk module on (the e2e job's
// service-desk preset).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** Goes to `url` and waits until the app is hydrated: a choice made in a
 * select before then (server-rendered, no handler yet) does nothing. */
async function open(page: Page, url: string) {
	await page.goto(url);
	await page.locator('html[data-hydrated]').waitFor({ state: 'attached' });
}

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

/** A client with a contact, and a ticket they raised: the ticket's id. */
async function ticketFrom(firstName: string, stamp: string): Promise<string> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `Canned ${stamp}`
		}
	});
	const clientId = ((await client.json()) as { id: string }).id;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName, lastName: 'Okafor' }
	});
	const contactId = ((await contact.json()) as { id: string }).id;
	const ticket = await api.post('/api/v1/tickets', {
		data: { orgId: clientId, title: `Locked out ${stamp}`, requesterId: contactId }
	});
	expect(ticket.status()).toBe(201);
	const id = ((await ticket.json()) as { id: string }).id;
	await api.dispose();
	return id;
}

test.describe('canned responses', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('a response is written once and filled in on a ticket', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const name = `Password reset ${stamp}`;
		const ticket = await ticketFrom('Chidi', stamp);
		await signIn(page);

		await open(page, '/settings/responses');
		await page.getByRole('button', { name: 'New response' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name').fill(name);
		await dialog.getByLabel('Category').fill('Accounts');
		await dialog
			.getByLabel('Text')
			.fill('Hi {{requester.first_name}}, your password is reset ({{ticket.reference}}).');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Response saved.')).toBeVisible();
		await expect(page.getByText(name)).toBeVisible();

		await open(page, `/tickets/${ticket}`);
		await page.getByLabel('Insert a response').selectOption({ label: `Accounts · ${name}` });
		const body = page.locator('#note-body');
		await expect(body).toHaveValue(/^Hi Chidi, your password is reset \([A-Z]+-\d+\)\.$/);
		await page.getByRole('button', { name: 'Reply' }).click();
		await expect(page.getByText('Reply added.')).toBeVisible();
		await expect(page.getByText(/Hi Chidi, your password is reset/)).toBeVisible();
	});

	test('a template fills in a new ticket', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		await ticketFrom('Priya', stamp);
		const name = `New starter ${stamp}`;
		await signIn(page);

		await open(page, '/settings/templates');
		await page.getByRole('button', { name: 'New template' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name').fill(name);
		await dialog.getByLabel('Type').selectOption({ label: 'Service request' });
		await dialog.getByLabel('Impact').selectOption({ label: 'Low' });
		await dialog.getByLabel('Title').fill('New starter: ');
		await dialog.getByLabel('Description').fill('Laptop, accounts and licences.');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Template saved.')).toBeVisible();

		await open(page, '/tickets/new');
		// Each choice reloads the form (a choice before the page is ready is
		// lost: choose until the URL has it).
		await expect(async () => {
			await page.getByLabel('Template').selectOption({ label: name });
			await expect(page).toHaveURL(/template=/, { timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		await page.getByLabel('Client').selectOption({ label: `Canned ${stamp}` });
		// The template survives choosing the client.
		await expect(page).toHaveURL(/org=.*template=/);
		await expect(page.getByLabel('Description')).toHaveValue('Laptop, accounts and licences.');
		await expect(page.getByLabel('Type')).toHaveValue('service_request');
		await page.getByLabel('Title').fill(`New starter: Priya ${stamp}`);
		await page.getByRole('button', { name: 'Create ticket' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`New starter: Priya ${stamp}`);
		await expect(page.getByText('Service request').first()).toBeVisible();
	});
});
