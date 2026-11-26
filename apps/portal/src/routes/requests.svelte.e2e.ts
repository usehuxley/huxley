import { expect, request, test } from '@playwright/test';

// Full-stack with real email: the MSP offers a form in the portal, and a
// client contact asks for something with it and follows the request.
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

/** A portal form at the MSP and a client contact invited: their email. */
async function setUp(stamp: string) {
	const api = await staff();
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const root = orgs.find((o) => o.parentId === null)!.id;
	const form = await api.post('/api/v1/forms', {
		data: {
			orgId: root,
			form: {
				key: `new-starter-${stamp}`,
				name: `New starter ${stamp}`,
				description: 'An account and a laptop for someone joining.',
				portal: true,
				portalCategory: 'Accounts',
				schema: {
					type: 'object',
					required: ['name'],
					properties: { name: { type: 'string', title: 'Their name' } }
				}
			}
		}
	});
	expect(form.status(), await form.text()).toBe(201);
	const client = await api.post('/api/v1/orgs', {
		data: { parentId: root, kind: 'client', name: `Requests ${stamp}` }
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const email = `cy-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Cy', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_user' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	await api.dispose();
	return email;
}

test.describe('portal requests', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a contact asks for something with a portal form', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const email = await setUp(stamp);
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Cy?');

		await page.getByRole('link', { name: 'Request something' }).first().click();
		const accounts = page.getByRole('region', { name: 'Accounts' });
		await accounts.getByRole('link', { name: `New starter ${stamp}` }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`New starter ${stamp}`);
		// Required answers are asked for (the browser checks first).
		await expect(page.getByLabel('Their name')).toHaveAttribute('required', '');
		await page.getByLabel('Their name').fill('Dee Starter');
		await page.getByRole('button', { name: 'Send request' }).click();

		await expect(page.getByText('Thanks: your request is in.')).toBeVisible();
		const mine = page.getByRole('list', { name: 'Your requests' });
		await expect(mine.getByText(`New starter ${stamp}`)).toBeVisible();
		await expect(mine.getByText('Received')).toBeVisible();
	});
});
