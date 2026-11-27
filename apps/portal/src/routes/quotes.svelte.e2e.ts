import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack (ADR-065): a client approver opens a quote in the portal,
// takes an option, signs and accepts it; staff see the signature and the
// opportunity is won. Needs the service-desk preset, MAILPIT_URL and the
// API's portal URL set to this preview server.
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

async function json<T>(
	r: Promise<import('@playwright/test').APIResponse>,
	status = 201
): Promise<T> {
	const res = await r;
	expect(res.status(), await res.text()).toBe(status);
	return (await res.json()) as T;
}

test.describe('quotes in the portal', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the crm module is off in this run');

	test('a client takes an option, signs and accepts a quote', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const owner: APIRequestContext = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await owner.post('/api/v1/auth/login', {
			data: { email: ownerEmail, password: ownerPassword }
		});
		for (const module of ['contracts', 'sales']) {
			await json(owner.put(`/api/v1/modules/${module}`, { data: { enabled: true } }), 200);
		}
		const orgs = await json<{ id: string; parentId: string | null }[]>(
			owner.get('/api/v1/orgs'),
			200
		);
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = await json<{ id: string }>(
			owner.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Signco ${stamp}` }
			})
		);
		const email = `sam-${stamp}@signco.example`;
		const contact = await json<{ id: string }>(
			owner.post(`/api/v1/orgs/${client.id}/contacts`, {
				data: { firstName: 'Sam', lastName: 'Signer', email }
			})
		);
		await json(
			owner.post(`/api/v1/contacts/${contact.id}/portal-access`, {
				data: { role: 'client_approver' }
			})
		);
		const opp = await json<{ id: string }>(
			owner.post('/api/v1/opportunities', {
				data: { orgId: client.id, opportunity: { name: `Laptops ${stamp}` } }
			})
		);
		const quote = await json<{ id: string; version: number }>(
			owner.post('/api/v1/quotes', {
				data: {
					orgId: client.id,
					quote: {
						title: `Laptops for the new team ${stamp}`,
						opportunityId: opp.id,
						contactId: contact.id,
						terms: 'Payment within 30 days.',
						lines: [
							{ name: 'Laptop', quantity: 3, unitCost: '800', unitPrice: '1000' },
							{ name: 'Docking station', optional: true, unitCost: '120', unitPrice: '200' }
						]
					}
				}
			})
		);
		await json(
			owner.post(`/api/v1/quotes/${quote.id}/send`, { data: { version: quote.version } }),
			200
		);

		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await page.getByRole('link', { name: 'Quotes' }).click();
		await page.getByRole('link', { name: `Laptops for the new team ${stamp}` }).click();
		const totals = page.getByLabel('Totals');
		await expect(totals).toContainText('$3,000');
		await page.getByLabel(/Docking station/).check();
		await expect(totals).toContainText('$3,200');

		// Sign: a stroke across the pad.
		await page.getByLabel('Your full name').fill('Sam Signer');
		const pad = page.locator('canvas');
		await pad.scrollIntoViewIfNeeded();
		const box = (await pad.boundingBox())!;
		await page.mouse.move(box.x + 20, box.y + box.height / 2);
		await page.mouse.down();
		for (let i = 1; i <= 10; i++) {
			await page.mouse.move(box.x + 20 + i * 25, box.y + box.height / 2 + (i % 2 ? -15 : 15));
		}
		await page.mouse.up();
		await page.getByLabel(/I accept this quote/).check();
		await page.getByRole('button', { name: 'Accept and sign' }).click();
		await expect(page.getByText(/Accepted by Sam Signer/)).toBeVisible();

		// Staff see the signature; the deal is won.
		const signature = await json<{ name: string; hasImage: boolean }>(
			owner.get(`/api/v1/quotes/${quote.id}/signature`),
			200
		);
		expect(signature.name).toBe('Sam Signer');
		expect(signature.hasImage).toBe(true);
		const won = await json<{ status: string }>(owner.get(`/api/v1/opportunities/${opp.id}`), 200);
		expect(won.status).toBe('won');
		await owner.dispose();
	});
});
