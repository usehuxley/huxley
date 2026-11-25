import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the servicedesk module on (the e2e job's
// service-desk preset).
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

/** A client with a P1 and a P3 ticket: the client's id. */
async function clientWithTickets(stamp: string): Promise<string> {
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
			name: `Views ${stamp}`
		}
	});
	const orgId = ((await client.json()) as { id: string }).id;
	for (const [title, level] of [
		['Server down', 1],
		['New mouse', 3]
	] as const) {
		const res = await api.post('/api/v1/tickets', {
			data: { orgId, title: `${title} ${stamp}`, impact: level, urgency: level }
		});
		expect(res.status()).toBe(201);
	}
	await api.dispose();
	return orgId;
}

test.describe('saved ticket views', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('filters are saved as a view, opened again, and deleted', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const org = await clientWithTickets(stamp);
		const name = `P1 at a client ${stamp}`;
		await signIn(page);

		await page.goto(`/tickets?view=open&org=${org}`);
		const table = page.getByRole('table', { name: 'Tickets' });
		await expect(table).toContainText(`New mouse ${stamp}`);
		await page.getByLabel('Priority').selectOption('p1');
		await expect(table).not.toContainText(`New mouse ${stamp}`);
		await expect(table).toContainText(`Server down ${stamp}`);

		await page.getByRole('button', { name: 'Save view' }).click();
		await page.getByRole('dialog').getByLabel('Name').fill(name);
		await page.getByRole('dialog').getByRole('button', { name: 'Save' }).click();
		const tabs = page.getByRole('navigation', { name: 'Ticket views' });
		await expect(tabs.getByRole('link', { name })).toHaveAttribute('aria-current', 'page');

		// Elsewhere, then back through the tab: the same filters.
		await tabs.getByRole('link', { name: 'All open' }).click();
		await expect(page).not.toHaveURL(/saved=/);
		await tabs.getByRole('link', { name }).click();
		await expect(page.getByLabel('Priority')).toHaveValue('p1');
		await expect(table).toContainText(`Server down ${stamp}`);
		await expect(table).not.toContainText(`New mouse ${stamp}`);

		await page.getByRole('button', { name: 'Delete view' }).click();
		await expect(page).toHaveURL(/\/tickets$/);
		await expect(tabs.getByRole('link', { name })).toHaveCount(0);
	});
});
