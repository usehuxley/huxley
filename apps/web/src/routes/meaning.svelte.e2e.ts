import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import {
	expect,
	request,
	test,
	type APIRequestContext,
	type APIResponse,
	type Page
} from '@playwright/test';

// Full-stack: needs the API, the servicedesk module on (the e2e job's
// service-desk preset) and pgvector in the database. A client gets its own
// embeddings provider (a mock below, on 127.0.0.1, which the e2e server may
// call) with a model name only this test uses, so other tests' embeddings
// never mix in.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** A toy embedding: how much a text is about printers, networks and email. */
function meaning(text: string): number[] {
	const t = text.toLowerCase();
	const about = (words: string[]) => (words.some((w) => t.includes(w)) ? 1 : 0.05);
	return [
		about(['print', 'toner', 'paper']),
		about(['vpn', 'network', 'wifi']),
		about(['mail', 'outlook', 'inbox'])
	];
}

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

async function ok(response: Promise<APIResponse>, status = 200) {
	const r = await response;
	expect(r.status(), await r.text()).toBe(status);
	return r.json() as Promise<{ id: string; status?: string }>;
}

/** Embeds the client's tickets (or articles) now: an embed step, run at the client. */
async function embed(
	api: APIRequestContext,
	orgId: string,
	stamp: string,
	step = 'core.ticket.embed'
) {
	const workflow = await ok(
		api.post('/api/v1/workflows', {
			data: {
				orgId,
				document: {
					apiVersion: 'usehuxley.io/v1',
					kind: 'Workflow',
					metadata: {
						key: `${step.replaceAll('.', '-')}-${stamp}`,
						name: `Embed ${step} ${stamp}`
					},
					spec: { steps: [{ id: 'embed', type: step }] }
				}
			}
		}),
		201
	);
	await ok(api.post(`/api/v1/workflows/${workflow.id}/publish`, { data: {} }));
	const run = await ok(api.post(`/api/v1/workflows/${workflow.id}/runs`, { data: {} }), 201);
	await expect
		.poll(async () => (await ok(api.get(`/api/v1/runs/${run.id}`))).status, { timeout: 20_000 })
		.toBe('succeeded');
}

test.describe('tickets by meaning', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('tickets about the same thing are found in other words', async ({ page, context }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const model = createServer((req, res) => {
			let body = '';
			req.on('data', (chunk) => (body += chunk));
			req.on('end', () => {
				const { input } = JSON.parse(body) as { input: string[] };
				res.writeHead(200, { 'content-type': 'application/json' });
				res.end(
					JSON.stringify({
						model: `embed-${stamp}`,
						data: input.map((text, index) => ({ index, embedding: meaning(text) })),
						usage: { prompt_tokens: 1, total_tokens: 1 }
					})
				);
			});
		});
		await new Promise<void>((resolve) => model.listen(0, '127.0.0.1', resolve));
		const { port } = model.address() as AddressInfo;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		try {
			await api.post('/api/v1/auth/login', {
				data: { email: ownerEmail, password: ownerPassword }
			});
			const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
				id: string;
				parentId: string | null;
			}[];
			const client = await ok(
				api.post('/api/v1/orgs', {
					data: {
						parentId: orgs.find((o) => o.parentId === null)!.id,
						kind: 'client',
						name: `Meaning ${stamp}`
					}
				}),
				201
			);
			await ok(
				api.put('/api/v1/ai/provider', {
					data: {
						orgId: client.id,
						use: 'embeddings',
						provider: {
							kind: 'openai_compatible',
							model: `embed-${stamp}`,
							baseUrl: `http://127.0.0.1:${port}/v1`,
							apiKey: 'sk-e2e-embed'
						}
					}
				})
			);
			const tickets: string[] = [];
			for (const [title, description] of [
				['Second floor', 'The toner light is on and nothing comes out'],
				['Working from home', 'The VPN drops every hour'],
				['Outlook', 'Inbox will not sync']
			]) {
				const t = await ok(
					api.post('/api/v1/tickets', {
						data: { orgId: client.id, title: `${title} ${stamp}`, description }
					}),
					201
				);
				tickets.push(t.id);
			}
			// Published articles at the client: about printers and about email.
			for (const [title, body] of [
				[`Toner ${stamp}`, 'Replace the toner cartridge'],
				[`Mail ${stamp}`, 'Rebuild the Outlook profile']
			]) {
				const a = await ok(
					api.post('/api/v1/kb/articles', { data: { orgId: client.id, title, body } }),
					201
				);
				await ok(api.post(`/api/v1/kb/articles/${a.id}/publish`, { data: {} }));
			}
			await embed(api, client.id, stamp);
			await embed(api, client.id, stamp, 'core.kb.embed');

			// Searching as the client (its embeddings provider reads the words).
			await signIn(page);
			await context.addCookies([
				{ name: 'hx_org', value: client.id, url: new URL(page.url()).origin }
			]);
			await page.goto('/tickets');
			await page.getByRole('link', { name: 'By meaning' }).click();
			await page.getByLabel("Describe what you're looking for").fill('the printer is out of paper');
			await page.getByRole('button', { name: 'Search', exact: true }).click();
			const found = page.getByRole('list', { name: 'Tickets by meaning' }).getByRole('listitem');
			await expect(found).toHaveCount(3);
			await expect(found.first()).toContainText(`Second floor ${stamp}`);
			await expect(found.first()).toContainText('100%');

			await page.getByLabel("Describe what you're looking for").fill('wifi keeps dropping');
			await page.getByRole('button', { name: 'Search', exact: true }).click();
			await expect(found.first()).toContainText(`Working from home ${stamp}`);

			// Articles by meaning.
			await page.goto('/knowledge');
			await page.getByRole('link', { name: 'By meaning' }).click();
			await page.getByLabel("Describe what you're looking for").fill('my inbox is empty');
			await page.getByRole('button', { name: 'Search', exact: true }).click();
			const articles = page
				.getByRole('list', { name: 'Articles by meaning' })
				.getByRole('listitem');
			await expect(articles.first()).toContainText(`Mail ${stamp}`);

			// The printer ticket suggests the toner article, by its own meaning.
			await page.goto(`/tickets/${tickets[0]}`);
			const suggested = page
				.getByRole('list', { name: 'Suggested articles' })
				.getByRole('listitem');
			await expect(suggested.first()).toContainText(`Toner ${stamp}`);
			await expect(suggested.first()).toContainText('About the same thing (100%)');
		} finally {
			await api.dispose();
			model.close();
		}
	});
});
