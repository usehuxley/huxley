import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-078): a client approver sees the company's invoice in
// the portal, due, and downloads it. Needs the service-desk preset,
// MAILPIT_URL and the API's portal URL set to this preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function linkIn(to: string): Promise<string> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 50; i++) {
			const search = await api.get(`/api/v1/search?query=${encodeURIComponent(`to:"${to}"`)}`);
			const { messages } = (await search.json()) as { messages: { ID: string }[] };
			for (const { ID } of messages) {
				const message = (await (await api.get(`/api/v1/message/${ID}`)).json()) as {
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

async function ok<T>(r: Promise<import('@playwright/test').APIResponse>): Promise<T> {
	const res = await r;
	expect(res.ok(), await res.text()).toBeTruthy();
	return (await res.json()) as T;
}

test.describe('invoices in the portal', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'tickets are off in this run');

	test('a client sees an invoice that went out, and downloads it', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const today = new Date().toISOString().slice(0, 10);
		const owner = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await owner.post('/api/v1/auth/login', {
			data: { email: ownerEmail, password: ownerPassword }
		});
		for (const module of ['contracts', 'billing']) {
			await ok(owner.put(`/api/v1/modules/${module}`, { data: { enabled: true } }));
		}
		const orgs = await ok<{ id: string; parentId: string | null }[]>(owner.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		await ok(
			owner.put('/api/v1/billing/settings', {
				data: {
					orgId: root,
					hourlyRate: '120',
					currency: 'USD',
					paymentDays: 30,
					approvedTimeOnly: false
				}
			})
		);
		const client = await ok<{ id: string }>(
			owner.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Payco ${stamp}` }
			})
		);
		const email = `pat-${stamp}@payco.example`;
		const contact = await ok<{ id: string }>(
			owner.post(`/api/v1/orgs/${client.id}/contacts`, {
				data: { firstName: 'Pat', lastName: 'Payer', email }
			})
		);
		await ok(
			owner.post(`/api/v1/contacts/${contact.id}/portal-access`, {
				data: { role: 'client_approver' }
			})
		);
		const ticket = await ok<{ id: string }>(
			owner.post('/api/v1/tickets', { data: { orgId: client.id, title: `Printer ${stamp}` } })
		);
		const types = await ok<{ id: string; name: string }[]>(owner.get('/api/v1/work-types'));
		await ok(
			owner.post(`/api/v1/tickets/${ticket.id}/time`, {
				data: {
					workTypeId: types.find((t) => t.name === 'Remote support')!.id,
					workedOn: today,
					minutes: 60
				}
			})
		);
		const run = await ok<{ id: string }>(
			owner.post('/api/v1/billing/runs', {
				data: { orgId: root, from: today, to: today, clients: [client.id] }
			})
		);
		const [invoice] = await ok<{ id: string; number: number; version: number }[]>(
			owner.get(`/api/v1/invoices?runId=${run.id}`)
		);
		await ok(
			owner.post(`/api/v1/invoices/${invoice.id}/approve`, { data: { version: invoice.version } })
		);
		await ok(owner.post(`/api/v1/invoices/${invoice.id}/sent`));
		await owner.dispose();

		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await page.getByRole('link', { name: 'Invoices' }).click();
		const list = page.getByRole('list', { name: 'Invoices' });
		const row = list.getByRole('listitem').filter({ hasText: `INV-${invoice.number}` });
		await expect(row).toContainText('Due');
		const download = page.waitForEvent('download');
		await row.getByRole('link', { name: `Download INV-${invoice.number}` }).click();
		expect((await download).suggestedFilename()).toBe(`INV-${invoice.number}.pdf`);
	});
});
