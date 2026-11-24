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
});
