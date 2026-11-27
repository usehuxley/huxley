import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-053): "Ask the assistant" on a ticket opens a
// conversation about it: the model's instructions name the ticket.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('assistant context', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('asked from a ticket, the assistant knows which', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		// An OpenAI-compatible model that says whether its instructions named the ticket.
		const model = createServer((req, res) => {
			let body = '';
			req.on('data', (chunk: Buffer) => (body += chunk.toString()));
			req.on('end', () => {
				const parsed = JSON.parse(body) as {
					stream?: boolean;
					messages: { role: string; content: unknown }[];
				};
				const system = JSON.stringify(
					parsed.messages.find((m) => m.role === 'system')?.content ?? ''
				);
				const text = system.includes(`Printer jams ${stamp}`)
					? 'This is about the printer ticket.'
					: 'I do not know which ticket.';
				if (parsed.stream) {
					res.setHeader('content-type', 'text/event-stream');
					res.write(
						`data: ${JSON.stringify({ model: 'mock', choices: [{ index: 0, delta: { content: text } }] })}\n\n`
					);
					res.write(
						`data: ${JSON.stringify({ model: 'mock', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] })}\n\n`
					);
					res.end('data: [DONE]\n\n');
					return;
				}
				res.setHeader('content-type', 'application/json');
				res.end(
					JSON.stringify({
						model: 'mock',
						choices: [{ message: { content: text }, finish_reason: 'stop' }],
						usage: { prompt_tokens: 1, completion_tokens: 1 }
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
			const client = (await (
				await api.post('/api/v1/orgs', {
					data: {
						parentId: orgs.find((o) => o.parentId === null)!.id,
						kind: 'client',
						name: `Ask ${stamp}`
					}
				})
			).json()) as { id: string };
			const set = await api.put('/api/v1/ai/provider', {
				data: {
					orgId: client.id,
					provider: {
						kind: 'openai_compatible',
						model: 'mock',
						baseUrl: `http://127.0.0.1:${port}/v1`,
						apiKey: 'sk-e2e-mock'
					}
				}
			});
			expect(set.status(), await set.text()).toBe(200);
			const ticket = (await (
				await api.post('/api/v1/tickets', {
					data: { orgId: client.id, title: `Printer jams ${stamp}` }
				})
			).json()) as { id: string };

			await page.goto('/login');
			await page.getByLabel('Email').fill(ownerEmail!);
			await page.getByLabel('Password').fill(ownerPassword!);
			await page.getByRole('button', { name: 'Sign in', exact: true }).click();
			await expect(page.getByRole('heading', { level: 1 })).toHaveText(
				/^Good (morning|afternoon|evening), /
			);
			await page.goto(`/tickets/${ticket.id}`);
			await page.getByRole('link', { name: 'Ask the assistant' }).click();
			await expect(page.getByRole('note')).toContainText(`Printer jams ${stamp}`);
			await page.getByLabel('Message').fill('What is this about?');
			await page.getByRole('button', { name: 'Ask', exact: true }).click();
			await expect(page.getByRole('list', { name: 'Messages' })).toContainText(
				'This is about the printer ticket.'
			);
		} finally {
			await api.dispose();
			model.closeAllConnections();
			model.close();
		}
	});
});
