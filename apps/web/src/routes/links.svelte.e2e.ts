import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the servicedesk module on (the e2e job's
// service-desk preset).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

type Ticket = { id: string; reference: string };

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** Tickets with these titles, for one new client. */
async function tickets(titles: string[], stamp: string): Promise<Ticket[]> {
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
			name: `Links ${stamp}`
		}
	});
	const orgId = ((await client.json()) as { id: string }).id;
	const made: Ticket[] = [];
	for (const title of titles) {
		const res = await api.post('/api/v1/tickets', { data: { orgId, title: `${title} ${stamp}` } });
		expect(res.status()).toBe(201);
		made.push((await res.json()) as Ticket);
	}
	await api.dispose();
	return made;
}

test.describe('linked and merged tickets', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('tickets are linked as parent and child, and a duplicate is merged', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const [outage, symptom, dup] = await tickets(
			['Email outage', "Can't send", "Can't send!"],
			stamp
		);
		await signIn(page);

		// The symptom's parent is the outage.
		await page.goto(`/tickets/${symptom.id}`);
		await page.getByLabel('Ticket to link').fill(outage.reference);
		await page.getByLabel('Link as').selectOption('parent');
		await page.getByRole('button', { name: 'Link', exact: true }).click();
		await expect(page.getByText('Linked.')).toBeVisible();
		const linked = page.getByRole('list', { name: 'Linked tickets' });
		await expect(linked).toContainText(`Parent`);
		await expect(linked).toContainText(outage.reference);
		// And the outage shows it as a child.
		await linked.getByRole('link', { name: new RegExp(outage.reference) }).click();
		await expect(page.getByRole('list', { name: 'Linked tickets' })).toContainText('Child');

		// The duplicate is merged into the symptom: we land there.
		await page.goto(`/tickets/${dup.id}`);
		await page.getByRole('button', { name: 'Merge' }).click();
		await page.getByRole('dialog').getByLabel('Merge into').fill(symptom.reference);
		await page.getByRole('dialog').getByRole('button', { name: 'Merge' }).click();
		await expect(page).toHaveURL(new RegExp(`/tickets/${symptom.id}$`));
		await expect(page.getByText(`merged ${dup.reference} into this ticket`)).toBeVisible();
		await page.goto(`/tickets/${dup.id}`);
		await expect(page.getByText(`Merged into ${symptom.reference}.`)).toBeVisible();
		await expect(page.getByRole('button', { name: 'Merge' })).toBeHidden();
	});

	test('time is logged on a ticket and deleted', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const [ticket] = await tickets(['Slow laptop'], stamp);
		await signIn(page);
		await page.goto(`/tickets/${ticket.id}`);
		await page.getByLabel('Time spent').fill('1h 15m');
		await page.getByLabel('Work type').selectOption({ label: 'Onsite' });
		await page.getByLabel('Time notes').fill('Replaced the SSD.');
		await page.getByRole('button', { name: 'Log time' }).click();
		await expect(page.getByText('Time logged.')).toBeVisible();
		await expect(page.getByText('1h 15m logged')).toBeVisible();
		const entries = page.getByRole('list', { name: 'Time entries' });
		await expect(entries).toContainText('Onsite');
		await expect(entries).toContainText('Replaced the SSD.');
		// Not a duration: said so, nothing logged.
		await page.getByLabel('Time spent').fill('a while');
		await page.getByRole('button', { name: 'Log time' }).click();
		await expect(page.getByText('A duration, like 30m or 1h 15m.')).toBeVisible();
		await entries.getByRole('button', { name: /^Delete 1h 15m/ }).click();
		await expect(page.getByText('None logged.')).toBeVisible();
	});

	test('an appointment is booked on a ticket and shows on the dispatch board', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const [ticket] = await tickets(['Onsite visit'], stamp);
		await signIn(page);
		await page.goto(`/tickets/${ticket.id}`);
		await page.getByLabel('Day').fill('2099-03-04');
		await page.getByLabel('Starts').fill('10:00');
		await page.getByLabel('How long').fill('1h 30m');
		await page.getByLabel('Appointment notes').fill(`Bring a ladder ${stamp}`);
		await page.getByRole('button', { name: 'Book' }).click();
		await expect(page.getByText('Appointment booked.')).toBeVisible();
		const booked = page.getByRole('list', { name: 'Appointments' });
		await expect(booked).toContainText(`Bring a ladder ${stamp}`);

		await page.goto('/dispatch?week=2099-03-02');
		const board = page.getByRole('table', { name: 'Dispatch board' });
		const me = board.getByRole('row').filter({ hasText: 'E2E User' });
		await expect(me.getByRole('link', { name: new RegExp(ticket.reference) })).toBeVisible();

		await page.goto(`/tickets/${ticket.id}`);
		await booked.getByRole('button', { name: /^Cancel E2E User/ }).click();
		await expect(page.getByText('None booked.')).toBeVisible();
	});
});
