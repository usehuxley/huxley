import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
// Runs only where the servicedesk module is on (the e2e job's service-desk
// preset); with it off the suite runs as an install without it.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
// The preset the e2e instance was bootstrapped with (the CI job's matrix).
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** A new client organization, named `name`. */
async function newClient(name: string): Promise<string> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const created = await api.post('/api/v1/orgs', {
		data: { parentId: orgs.find((o) => o.parentId === null)!.id, kind: 'client', name }
	});
	expect(created.status()).toBe(201);
	const id = ((await created.json()) as { id: string }).id;
	await api.dispose();
	return id;
}

test.describe('service desk', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('a ticket is raised, worked and resolved', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const client = `Initech ${stamp}`;
		const queue = `Helpdesk ${stamp}`;
		const title = `Stapler missing ${stamp}`;
		await newClient(client);
		await signIn(page);

		// A queue to put it in.
		await page.goto('/settings/queues');
		await page.getByLabel('Name', { exact: true }).fill(queue);
		await page.getByRole('button', { name: 'Create queue' }).click();
		await expect(page.getByRole('link', { name: queue })).toBeVisible();

		// Raise it from the Tickets page.
		await page.getByRole('link', { name: 'Tickets', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Tickets');
		await page.getByRole('link', { name: 'New ticket' }).click();
		await page.getByLabel('Client').selectOption({ label: client });
		await page.getByLabel('Title').fill(title);
		await page.getByLabel('Description').fill('Red Swingline, last seen at my desk.');
		await page.getByLabel('Impact').selectOption({ label: 'High' });
		await page.getByLabel('Urgency').selectOption({ label: 'High' });
		await page.getByLabel('Queue').selectOption({ label: queue });
		await page.getByRole('button', { name: 'Create ticket' }).click();

		await expect(page.getByRole('heading', { level: 1 })).toHaveText(title);
		// Priority from the matrix: high impact and urgency.
		await expect(page.locator('span', { hasText: 'P1 · Critical' })).toBeVisible();
		await expect(page.getByText('Works out to P1 · Critical.')).toBeVisible();

		// Work it: assign, reply, add an internal note.
		await page.getByRole('button', { name: 'Assign to me' }).click();
		await expect(page.getByText('Assigned to you.')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Assign to me' })).toBeHidden();
		await page.getByRole('textbox', { name: 'Reply' }).fill('Looking for it now.');
		await page.getByRole('button', { name: 'Reply' }).click();
		await expect(page.getByText('Looking for it now.')).toBeVisible();
		// The default SLA: the reply met the first-response target.
		const sla = page
			.locator('section')
			.filter({ has: page.getByRole('heading', { name: 'SLA', exact: true }) });
		await expect(sla.getByText('Standard')).toBeVisible();
		await expect(sla.getByText('Met')).toBeVisible();
		await expect(page.getByText('Not yet')).toBeHidden();
		await page.getByRole('radio', { name: 'Internal note' }).check();
		await page.getByRole('textbox', { name: 'Internal note' }).fill('Check the basement.');
		await page.getByRole('button', { name: 'Add note' }).click();
		await expect(page.getByText('Check the basement.')).toBeVisible();
		await expect(page.getByText('Internal', { exact: true })).toBeVisible();

		// Resolving needs a resolution.
		await page.getByLabel('Status').selectOption({ label: 'Resolved' });
		await page.getByLabel('Resolution').fill('Found it in the basement.');
		await page.getByRole('button', { name: 'Change status' }).click();
		await expect(page.getByText('Status changed.')).toBeVisible();
		await expect(page.getByText('Found it in the basement.')).toBeVisible();
		await expect(page.getByText(/changed the status from New to Resolved/)).toBeVisible();

		// It's under Resolved now, not open.
		await page.getByRole('link', { name: '← Tickets' }).click();
		await page.getByRole('link', { name: 'All open' }).click();
		await expect(page).toHaveURL(/view=open/);
		await page.getByLabel('Search tickets').fill(stamp);
		await page.getByLabel('Search tickets').press('Enter');
		await expect(page.getByText('No tickets match')).toBeVisible();
		await page.getByRole('link', { name: 'Resolved', exact: true }).click();
		await expect(page).toHaveURL(/view=resolved/);
		await expect(page.getByRole('link', { name: title })).toBeVisible();
	});

	test('an SLA policy is set up and chosen for a client', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const client = `Chotchkie ${stamp}`;
		const policy = `Gold ${stamp}`;
		const orgId = await newClient(client);
		await signIn(page);

		// A round-the-clock policy with P3 targets.
		await page.goto('/settings/sla');
		await page.getByRole('button', { name: 'New policy' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name').fill(policy);
		await dialog.getByLabel('P3 first response').fill('soon');
		await dialog.getByRole('radio', { name: /Around the clock/ }).check();
		await dialog.getByRole('button', { name: 'Save policy' }).click();
		await expect(page.getByText('Some targets are not durations.').first()).toBeVisible();
		await dialog.getByLabel('P3 first response').fill('2h');
		await dialog.getByLabel('P3 resolution').fill('1d');
		await dialog.getByRole('button', { name: 'Save policy' }).click();
		await expect(page.getByText('Policy saved.')).toBeVisible();
		const targets = page.getByRole('table', { name: `${policy} targets` });
		await expect(targets.getByRole('row', { name: /P3 2h 24h/ })).toBeVisible();

		// Chosen for the client on its settings page.
		await page.goto(`/settings/organizations/${orgId}`);
		await expect(page.getByText(/Follows the default/)).toBeVisible();
		const card = page
			.locator('section')
			.filter({ has: page.getByRole('heading', { name: 'SLA policy', exact: true }) });
		await card.getByLabel('Policy').selectOption({ label: policy });
		await card.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('SLA policy set.')).toBeVisible();

		// A new ticket there is opened under it.
		await page.goto(`/tickets/new?org=${orgId}`);
		await page.getByLabel('Title').fill(`Flair ${stamp}`);
		await page.getByRole('button', { name: 'Create ticket' }).click();
		const sla = page
			.locator('section')
			.filter({ has: page.getByRole('heading', { name: 'SLA', exact: true }) });
		await expect(sla.getByText(policy)).toBeVisible();
		await expect(sla.getByText(/Due in (1|2) hours?/)).toBeVisible();
	});
});
