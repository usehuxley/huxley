import { expect, request, test } from '@playwright/test';

// Full-stack with real email: a client admin adds a colleague with access
// (they're emailed an invitation), gives another access, and removes it.
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

/** A client with an admin (invited) and a colleague without access: the admin's email. */
async function setUp(stamp: string) {
	const api = await staff();
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `People ${stamp}`
		}
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const contact = async (first: string) => {
		const r = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
			data: {
				firstName: first,
				lastName: stamp,
				email: `${first.toLowerCase()}-${stamp}@example.com`
			}
		});
		return ((await r.json()) as { id: string }).id;
	};
	const hal = await contact('Hal');
	await contact('Ivy');
	const invited = await api.post(`/api/v1/contacts/${hal}/portal-access`, {
		data: { role: 'client_admin' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	await api.dispose();
	return `hal-${stamp}@example.com`;
}

test.describe('portal people', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a client admin manages who has access', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const email = await setUp(stamp);
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Hal?');

		await page.getByRole('link', { name: 'People' }).first().click();
		const people = page.getByRole('list', { name: 'People' });
		await expect(people.getByText(`Hal ${stamp}`)).toBeVisible();

		// A new colleague, with access: they're emailed.
		await page.getByLabel('First name').fill('Jo');
		await page.getByLabel('Last name').fill(stamp);
		await page.getByLabel('Work email').fill(`jo-${stamp}@example.com`);
		await page.getByRole('button', { name: 'Add', exact: true }).click();
		await expect(page.getByText('Jo will get an email to sign in.')).toBeVisible();
		expect(await linkIn(`jo-${stamp}@example.com`)).toContain('/auth/link?token=');

		// Ivy gets access, then loses it.
		const ivy = people.getByRole('listitem').filter({ hasText: `Ivy ${stamp}` });
		await ivy.getByLabel(`Access for Ivy ${stamp}`).selectOption('client_approver');
		await ivy.getByRole('button', { name: 'Give access' }).click();
		await expect(page.getByText(`Ivy ${stamp} will get an email to sign in.`)).toBeVisible();
		await ivy.getByRole('button', { name: 'Remove access' }).click();
		await expect(page.getByText(`Ivy ${stamp} can no longer sign in.`)).toBeVisible();
		await expect(ivy.getByRole('button', { name: 'Restore access' })).toBeVisible();
	});
});
