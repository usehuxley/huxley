import { expect, request, test } from '@playwright/test';

// Full-stack with real email: staff invite a contact through the API, the
// invitation arrives in Mailpit (MAILPIT_URL), and the contact signs in to
// the portal with it. Needs the service-desk preset (crm, servicedesk,
// portal) and the API's portal URL set to this preview server. CI runs it.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** The portal sign-in link in the newest email to `to`. */
async function linkIn(to: string): Promise<string> {
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
				if (match) return match[0];
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no email to ${to}`);
	} finally {
		await api.dispose();
	}
}

/** A client with a contact invited to the portal: the contact's email. */
async function invite(stamp: string): Promise<string> {
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
			name: `Portal client ${stamp}`
		}
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const email = `ada-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Ada', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_user' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	expect(((await invited.json()) as { invitationSent: boolean }).invitationSent).toBe(true);
	await api.dispose();
	return email;
}

test.describe('portal tickets', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal and service desk are off in this run');

	test('an invited contact signs in, raises a ticket and replies', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const email = await invite(stamp);
		const link = new URL(await linkIn(email));

		// The link opens here and asks before signing in.
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Ada?');

		await page.getByRole('link', { name: 'Raise a ticket' }).first().click();
		await page.getByLabel('What do you need?').fill(`Laptop won’t start ${stamp}`);
		await page.getByLabel('Details').fill('Black screen since this morning.');
		await page.getByRole('button', { name: 'Raise ticket' }).click();
		await expect(page.getByText('Thanks: your ticket is raised.', { exact: false })).toBeVisible();

		await page.getByLabel('Reply').fill('It beeps three times.');
		await page.getByRole('button', { name: 'Send reply' }).click();
		await expect(page.getByRole('list', { name: 'Conversation' })).toContainText(
			'It beeps three times.'
		);

		await page.getByRole('link', { name: 'Tickets', exact: true }).first().click();
		await expect(page.getByRole('list', { name: 'Tickets' })).toContainText(
			`Laptop won’t start ${stamp}`
		);

		// Signed out, the portal asks again.
		await page.getByRole('button', { name: 'Sign out' }).click();
		await expect(page).toHaveURL(/\/login$/);
	});
});
