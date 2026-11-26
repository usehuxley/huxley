import { expect, request, test } from '@playwright/test';

// Full-stack with real email: the MSP publishes a dashboard to the portal,
// and a client admin sees it with their company's numbers. Set
// SCREENSHOT_DIR to keep a screenshot.
// Needs the service-desk preset, MAILPIT_URL and the API's portal URL set
// to this preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** The newest portal sign-in link emailed to `to`, other than `not`. */
async function linkIn(to: string, not?: string): Promise<string> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 50; i++) {
			const search = await api.get(`/api/v1/search?query=${encodeURIComponent(`to:"${to}"`)}`);
			const { messages } = (await search.json()) as { messages: { ID: string }[] };
			if (messages.length > 0) {
				const message = (await (await api.get(`/api/v1/message/${messages[0].ID}`)).json()) as {
					Text: string;
				};
				const match = message.Text.match(/https?:\/\/\S+\/auth\/link\?token=\S+/);
				if (match && match[0] !== not) return match[0];
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no new email to ${to}`);
	} finally {
		await api.dispose();
	}
}

async function staff() {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	return api;
}

const shots = process.env.SCREENSHOT_DIR;

/** A portal dashboard at the MSP, and a client with tickets and an admin (invited): their email. */
async function setUp(stamp: string) {
	const api = await staff();
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const root = orgs.find((o) => o.parentId === null)!.id;
	const count = [{ fn: 'count' }];
	const board = await api.post('/api/v1/dashboards', {
		data: {
			orgId: root,
			shared: true,
			dashboard: {
				name: `Your service ${stamp}`,
				portal: true,
				range: 'this_year',
				widgets: [
					{
						id: 'raised',
						title: 'Tickets raised',
						viz: 'number',
						width: 4,
						spec: { source: 'tickets', measures: count }
					},
					{
						id: 'priority',
						title: 'By priority',
						viz: 'bar',
						width: 8,
						spec: { source: 'tickets', measures: count, dimensions: [{ field: 'priority' }] }
					}
				]
			}
		}
	});
	expect(board.status(), await board.text()).toBe(201);
	const client = await api.post('/api/v1/orgs', {
		data: { parentId: root, kind: 'client', name: `Reports ${stamp}` }
	});
	const { id: clientId } = (await client.json()) as { id: string };
	for (const priority of ['p1', 'p2', 'p2']) {
		const t = await api.post('/api/v1/tickets', {
			data: { orgId: clientId, title: `Issue ${priority}`, priority }
		});
		expect(t.status(), await t.text()).toBe(201);
	}
	const email = `gil-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Gil', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_admin' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	await api.dispose();
	return email;
}

test.describe('portal reports', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a client admin views a published dashboard', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const email = await setUp(stamp);
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Gil?');

		await page.getByRole('link', { name: 'Reports' }).first().click();
		await page
			.getByRole('list', { name: 'Reports' })
			.getByRole('link', { name: `Your service ${stamp}` })
			.click();
		await expect(
			page.getByRole('region', { name: 'Tickets raised' }).getByText('3', { exact: true })
		).toBeVisible();
		const byPriority = page.getByRole('region', { name: 'By priority' });
		await byPriority.getByRole('button', { name: 'Table' }).click();
		await expect(byPriority.getByRole('row', { name: /p2/ })).toContainText('2');
		if (shots) await page.screenshot({ path: `${shots}/portal-report.png`, fullPage: true });
	});
});
