import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test, type APIResponse } from '@playwright/test';

// Full-stack (ADR-050): with the AI ticket insights pack at a client and a
// stand-in AI provider, Refresh on a ticket shows its summary, the things
// it mentions and a next step.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function model(): Promise<{ port: number; close: () => void }> {
	const server = createServer((req, res) => {
		let body = '';
		req.on('data', (chunk) => (body += chunk));
		req.on('end', () => {
			const parsed = JSON.parse(body) as { model: string };
			const answer = {
				summary: 'The second-floor printer shows offline for everyone.',
				people: ['Rowan'],
				devices: ['PRN-2F'],
				applications: [],
				errors: ['Printer offline'],
				next_step: 'Power-cycle the printer and check its network cable.'
			};
			res.writeHead(200, { 'content-type': 'application/json' });
			res.end(
				JSON.stringify({
					model: parsed.model,
					stop_reason: 'end_turn',
					content: [{ type: 'text', text: JSON.stringify(answer) }],
					usage: { input_tokens: 50, output_tokens: 30 }
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

async function ok(response: Promise<APIResponse>, status = 201) {
	const r = await response;
	expect(r.status(), await r.text()).toBe(status);
	return (await r.json()) as unknown;
}

test.describe('ticket insights', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a ticket gets a summary on refresh', async ({ page }) => {
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
			const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
				id: string;
				parentId: string | null;
			}[];
			const client = (await ok(
				api.post('/api/v1/orgs', {
					data: {
						parentId: orgs.find((o) => o.parentId === null)!.id,
						kind: 'client',
						name: `Insights ${stamp}`
					}
				})
			)) as { id: string };
			await ok(
				api.put('/api/v1/ai/provider', {
					data: {
						orgId: client.id,
						use: 'chat',
						provider: {
							kind: 'anthropic',
							baseUrl: `http://127.0.0.1:${ai.port}`,
							apiKey: 'sk-ant-e2e'
						}
					}
				}),
				200
			);
			const review = (await ok(
				api.post(`/api/v1/packs/builtin/huxley/ai-ticket-insights?orgId=${client.id}`)
			)) as { uploadId: string; version: string };
			const installed = (await ok(
				api.post(`/api/v1/packs/uploads/${review.uploadId}/install`, {
					data: { orgId: client.id, version: review.version }
				})
			)) as { items: { workflows: Record<string, string> } };
			for (const id of Object.values(installed.items.workflows)) {
				await ok(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }), 200);
			}
			const ticket = (await ok(
				api.post('/api/v1/tickets', {
					data: { orgId: client.id, title: `Printer offline ${stamp}` }
				})
			)) as { id: string };

			await page.goto('/login');
			await page.getByLabel('Email').fill(ownerEmail!);
			await page.getByLabel('Password').fill(ownerPassword!);
			await page.getByRole('button', { name: 'Sign in', exact: true }).click();
			await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
			await page.goto(`/tickets/${ticket.id}`);
			await page.getByRole('button', { name: 'Refresh', exact: true }).click();
			await expect(page.getByText('Asked for a fresh summary', { exact: false })).toBeVisible();
			await expect(async () => {
				await page.reload();
				await expect(page.getByLabel('Summary')).toHaveText(
					'The second-floor printer shows offline for everyone.',
					{ timeout: 1000 }
				);
			}).toPass({ timeout: 30_000 });
			await expect(
				page.getByText('Power-cycle the printer and check its network cable.')
			).toBeVisible();
			await expect(page.getByText('PRN-2F')).toBeVisible();
		} finally {
			await api.dispose();
			ai.close();
		}
	});
});
