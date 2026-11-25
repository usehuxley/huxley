import { expect, request, test, type APIRequestContext, type Page } from '@playwright/test';

// Full-stack email to ticket and back: needs the API (with a worker, so the
// pack's workflows run), sending to Mailpit (MAILPIT_URL), and the
// servicedesk module on (the e2e job's service-desk preset).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

async function owner(): Promise<APIRequestContext> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	return api;
}

/** The root organization's id. */
async function root(api: APIRequestContext): Promise<string> {
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	return orgs.find((o) => o.parentId === null)!.id;
}

/** Installs the built-in Service desk email pack at the root (once). */
async function installPack(api: APIRequestContext, orgId: string) {
	const review = await api.post(`/api/v1/packs/builtin/huxley/servicedesk-email?orgId=${orgId}`);
	expect(review.status()).toBe(201);
	const { uploadId, version, installedVersion } = (await review.json()) as {
		uploadId: string;
		version: string;
		installedVersion: string | null;
	};
	if (installedVersion === version) return;
	const installed = await api.post(`/api/v1/packs/uploads/${uploadId}/install`, {
		data: { orgId, version }
	});
	expect(installed.status(), await installed.text()).toBe(201);
	// Pack workflows arrive as drafts: publish them, as an admin would.
	const { items } = (await installed.json()) as { items: { workflows: Record<string, string> } };
	for (const id of Object.values(items.workflows)) {
		const published = await api.post(`/api/v1/workflows/${id}/publish`, { data: {} });
		expect(published.status(), await published.text()).toBe(200);
	}
}

/** Waits for an email to `to` whose subject contains `subject`. */
async function expectEmail(to: string, subject: string) {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 100; i++) {
			const query = `to:"${to}" subject:"${subject}"`;
			const res = await api.get(`/api/v1/search?query=${encodeURIComponent(query)}`);
			if (((await res.json()) as { messages: unknown[] }).messages.length > 0) return;
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no "${subject}" email to ${to}`);
	} finally {
		await api.dispose();
	}
}

test.describe('mailboxes', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('email in becomes a ticket, and the reply goes back out', async ({ page }) => {
		test.setTimeout(90_000);
		const stamp = Date.now().toString(36);
		const requester = `ada.${stamp}@initech.example`;
		const mailbox = `help.${stamp}@msp.example`;
		const api = await owner();
		const rootId = await root(api);
		await installPack(api, rootId);
		// A client with Ada as a contact.
		const client = await api.post('/api/v1/orgs', {
			data: { parentId: rootId, kind: 'client', name: `Initech ${stamp}` }
		});
		const clientId = ((await client.json()) as { id: string }).id;
		const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
			data: { firstName: 'Ada', lastName: 'Lovelace', email: requester }
		});
		expect(contact.status()).toBe(201);

		// A forwarded mailbox, added in Settings.
		await signIn(page);
		await page.goto('/settings/mailboxes');
		await page.getByRole('button', { name: 'New mailbox' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name', { exact: true }).fill('Help desk');
		await dialog.getByLabel('Address', { exact: true }).fill(mailbox);
		await dialog.getByLabel('How mail arrives').selectOption('forward');
		await dialog.getByRole('button', { name: 'Add mailbox' }).click();
		await expect(page.getByRole('heading', { name: 'Help desk' })).toBeVisible();
		await expect(page.getByText('Nothing yet.')).toBeVisible();
		await expect(page.getByText(/curl -X POST .*\/messages/)).toBeVisible();
		const mailboxId = page.url().split('/').pop()!;

		// Ada writes in.
		const posted = await api.post(`/api/v1/mailboxes/${mailboxId}/messages`, {
			headers: { 'content-type': 'message/rfc822' },
			data: Buffer.from(
				`From: Ada Lovelace <${requester}>\r\nTo: ${mailbox}\r\nSubject: Engine won't start ${stamp}\r\n` +
					`Message-ID: <first.${stamp}@initech.example>\r\n\r\nIt makes a clicking noise.\r\n`
			)
		});
		expect(posted.status()).toBe(202);
		await api.dispose();

		// The pack's workflow opens a ticket; the mailbox page shows it.
		await expect(async () => {
			await page.reload();
			await expect(page.getByText('Ticket opened')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 30_000 });
		await page
			.getByRole('row', { name: new RegExp(`Engine won't start ${stamp}`) })
			.getByRole('link')
			.click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Engine won't start ${stamp}`);
		await expect(page.getByText('It makes a clicking noise.')).toBeVisible();

		// A reply goes back to Ada by email, with the ticket's token.
		await page.getByRole('textbox', { name: 'Reply' }).fill('Have you tried the crank?');
		await page.getByRole('button', { name: 'Reply' }).click();
		await expect(page.getByText('Reply added.')).toBeVisible();
		await expectEmail(requester, `Engine won't start ${stamp} [HX#`);
		await expect(async () => {
			await page.reload();
			await expect(page.getByText(`Emailed to ${requester}`)).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 20_000 });
	});

	test('a Microsoft 365 mailbox keeps its Graph settings', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const address = `help-${stamp}@contoso.example`;
		await signIn(page);
		await page.goto('/settings/mailboxes');
		await page.getByRole('button', { name: 'New mailbox' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name', { exact: true }).fill(`M365 ${stamp}`);
		await dialog.getByLabel('Address', { exact: true }).fill(address);
		await dialog.getByLabel('How mail arrives').selectOption('graph');
		await dialog.getByLabel('Microsoft Graph connection').fill('contoso');
		await dialog.getByRole('button', { name: 'Add mailbox' }).click();
		await expect(page.getByRole('heading', { name: `M365 ${stamp}` })).toBeVisible();
		await expect(page.getByLabel('How mail arrives')).toHaveValue('graph');
		// Saved, with the address and inbox as the defaults.
		await expect(page.getByLabel('Microsoft Graph connection')).toHaveValue('contoso');
		await expect(page.getByLabel('Mailbox', { exact: true })).toHaveValue('');
		await expect(page.getByLabel('Folder')).toHaveValue('');
	});
});
