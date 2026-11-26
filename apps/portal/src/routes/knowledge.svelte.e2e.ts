import { expect, request, test } from '@playwright/test';

// Full-stack with real email: the MSP publishes an article for clients, and
// a client contact finds it in the portal's help articles.
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

/** A published article for clients, and a client contact invited: their email. */
async function setUp(stamp: string) {
	const api = await staff();
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const root = orgs.find((o) => o.parentId === null)!.id;
	const created = await api.post('/api/v1/kb/articles', {
		data: {
			orgId: root,
			title: `Connect to the Wi-Fi ${stamp}`,
			body: `Pick **Guest-${stamp}** and enter the code at reception.`,
			visibility: 'clients'
		}
	});
	expect(created.status(), await created.text()).toBe(201);
	const { id: article } = (await created.json()) as { id: string };
	const published = await api.post(`/api/v1/kb/articles/${article}/publish`, { data: {} });
	expect(published.status(), await published.text()).toBe(200);
	const client = await api.post('/api/v1/orgs', {
		data: { parentId: root, kind: 'client', name: `Articles ${stamp}` }
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const email = `di-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Di', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_user' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	await api.dispose();
	return email;
}

test.describe('portal help articles', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a contact searches and reads an article', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const email = await setUp(stamp);
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Di?');

		await page.getByRole('link', { name: 'Help articles' }).first().click();
		await page.getByRole('search').getByRole('textbox').fill(`Guest-${stamp}`);
		await page.getByRole('button', { name: 'Search', exact: true }).click();
		const results = page.getByRole('list', { name: 'Articles' });
		await results.getByRole('link', { name: `Connect to the Wi-Fi ${stamp}` }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			`Connect to the Wi-Fi ${stamp}`
		);
		await expect(page.getByRole('document').getByText(`Guest-${stamp}`)).toBeVisible();
	});
});
