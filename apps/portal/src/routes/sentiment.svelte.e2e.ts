import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack (ADR-046): with the built-in AI sentiment pack at a client and
// a stand-in AI provider, a ticket raised in the portal and a reply are
// each judged and recorded. Needs the service-desk preset, MAILPIT_URL, the
// API's portal URL set to this preview server, and 127.0.0.1 allowed by
// the egress policy.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** The newest portal sign-in link emailed to `to`. */
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

/** Answers like Anthropic's Messages API: angry text is -2, the rest +1. */
async function model(): Promise<{ port: number; close: () => void }> {
	const server = createServer((req, res) => {
		let body = '';
		req.on('data', (chunk) => (body += chunk));
		req.on('end', () => {
			const parsed = JSON.parse(body) as { model: string; messages: { content: unknown }[] };
			const text = JSON.stringify(parsed.messages);
			const answer = /furious/i.test(text)
				? { score: -2, reason: 'Angry about repeated outages' }
				: { score: 1, reason: 'Thankful for the fix' };
			res.writeHead(200, { 'content-type': 'application/json' });
			res.end(
				JSON.stringify({
					model: parsed.model,
					stop_reason: 'end_turn',
					content: [{ type: 'text', text: JSON.stringify(answer) }],
					usage: { input_tokens: 20, output_tokens: 5 }
				})
			);
		});
	});
	await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
	return {
		port: (server.address() as AddressInfo).port,
		close: () => {
			server.closeAllConnections();
			server.close();
		}
	};
}

/** A client with the stand-in provider, the cx module and the pack; an invited contact's email. */
async function setUp(api: APIRequestContext, stamp: string, port: number) {
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `Tone ${stamp}`
		}
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const provider = await api.put('/api/v1/ai/provider', {
		data: {
			orgId: clientId,
			use: 'chat',
			provider: { kind: 'anthropic', baseUrl: `http://127.0.0.1:${port}`, apiKey: 'sk-ant-e2e' }
		}
	});
	expect(provider.status(), await provider.text()).toBe(200);
	expect((await api.put('/api/v1/modules/cx', { data: { enabled: true } })).status()).toBe(200);
	const review = await api.post(`/api/v1/packs/builtin/huxley/ai-sentiment?orgId=${clientId}`);
	expect(review.status(), await review.text()).toBe(201);
	const { uploadId, version } = (await review.json()) as { uploadId: string; version: string };
	const installed = await api.post(`/api/v1/packs/uploads/${uploadId}/install`, {
		data: { orgId: clientId, version }
	});
	expect(installed.status(), await installed.text()).toBe(201);
	const { items } = (await installed.json()) as { items: { workflows: Record<string, string> } };
	for (const id of Object.values(items.workflows)) {
		const published = await api.post(`/api/v1/workflows/${id}/publish`, { data: {} });
		expect(published.status(), await published.text()).toBe(200);
	}
	const email = `rowan-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Rowan', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_user' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	return email;
}

test.describe('sentiment', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal and service desk are off in this run');

	test('a client’s messages are judged and recorded', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const ai = await model();
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		try {
			await api.post('/api/v1/auth/login', {
				data: { email: ownerEmail, password: ownerPassword }
			});
			const email = await setUp(api, stamp, ai.port);
			const link = new URL(await linkIn(email));
			await page.goto(link.pathname + link.search);
			await page.getByRole('button', { name: 'Sign in' }).click();

			await page.getByRole('link', { name: 'Raise a ticket' }).first().click();
			await page.getByLabel('What do you need?').fill(`Outage again ${stamp}`);
			await page.getByLabel('Details').fill('We are furious: the third outage this week.');
			await page.getByRole('button', { name: 'Raise ticket' }).click();
			await expect(
				page.getByText('Thanks: your ticket is raised.', { exact: false })
			).toBeVisible();
			const ticketId = new URL(page.url()).pathname.split('/').pop()!;
			const judged = async () =>
				(await (await api.get(`/api/v1/tickets/${ticketId}/sentiment`)).json()) as {
					score: number;
					reason: string;
				}[];
			await expect
				.poll(async () => (await judged()).map((e) => e.score), { timeout: 20_000 })
				.toEqual([-2]);
			expect((await judged())[0].reason).toBe('Angry about repeated outages');

			await page.getByLabel('Reply').fill('Thanks, that fixed it.');
			await page.getByRole('button', { name: 'Send reply' }).click();
			await expect
				.poll(async () => (await judged()).map((e) => e.score), { timeout: 20_000 })
				.toEqual([-2, 1]);
		} finally {
			await api.dispose();
			ai.close();
		}
	});
});
