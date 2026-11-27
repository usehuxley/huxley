import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test, type APIResponse, type Page } from '@playwright/test';

// Full-stack: needs the API. A client gets its own stand-in AI provider (on
// 127.0.0.1, which the e2e server may call) and the Presales scoping pack.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

/** Answers like Anthropic's Messages API with a drafted scope. */
async function model() {
	const server = createServer((req, res) => {
		let body = '';
		req.on('data', (chunk) => (body += chunk));
		req.on('end', () => {
			const parsed = JSON.parse(body) as { model: string };
			const answer = {
				sections: [
					{ key: 'in_scope', body: '- Rewire the 30 desks on both floors', sources: [1] },
					{ key: 'deliverables', body: '- A labelled patch panel', sources: [1] }
				],
				questions: [{ section: 'timeline', question: 'Which weekend can the office close?' }]
			};
			res.writeHead(200, { 'content-type': 'application/json' });
			res.end(
				JSON.stringify({
					model: parsed.model,
					stop_reason: 'end_turn',
					content: [{ type: 'text', text: JSON.stringify(answer) }],
					usage: { input_tokens: 300, output_tokens: 60 }
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

async function ok<T>(response: Promise<APIResponse>, status = 0) {
	const r = await response;
	if (status) expect(r.status(), await r.text()).toBe(status);
	else expect(r.ok(), await r.text()).toBeTruthy();
	return (await r.json()) as T;
}

test.describe('presales scoping', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);

	test('discovery becomes an AI-drafted scope, approved and carried by the quote', async ({
		page
	}) => {
		test.setTimeout(90_000);
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
			for (const module of ['crm', 'contracts', 'sales']) {
				await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
			}
			const orgs = await ok<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
			const client = await ok<{ id: string }>(
				api.post('/api/v1/orgs', {
					data: {
						parentId: orgs.find((o) => o.parentId === null)!.id,
						kind: 'client',
						name: `Scoped ${stamp}`
					}
				})
			);
			await ok(
				api.put('/api/v1/ai/provider', {
					data: {
						orgId: client.id,
						use: 'chat',
						provider: {
							kind: 'anthropic',
							baseUrl: `http://127.0.0.1:${ai.port}`,
							apiKey: 'sk-e2e'
						}
					}
				})
			);
			const pack = await ok<{ uploadId: string; version: string }>(
				api.post(`/api/v1/packs/builtin/huxley/presales-scoping?orgId=${client.id}`)
			);
			const installed = await ok<{ items: { workflows: Record<string, string> } }>(
				api.post(`/api/v1/packs/uploads/${pack.uploadId}/install`, {
					data: { orgId: client.id, version: pack.version }
				})
			);
			for (const id of Object.values(installed.items.workflows)) {
				await ok(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }), 200);
			}
			const opp = await ok<{ id: string }>(
				api.post('/api/v1/opportunities', {
					data: { orgId: client.id, opportunity: { name: `Office rewire ${stamp}` } }
				})
			);

			await signIn(page);
			await page.goto(`/sales/opportunities/${opp.id}`);
			await page.getByRole('link', { name: 'Open scoping' }).click();
			await page.getByLabel('Kind of engagement').selectOption({ label: 'Project' });
			await page.getByRole('button', { name: 'Start', exact: true }).click();
			await expect(page.getByRole('heading', { level: 1 })).toHaveText('Scope of work');

			await page.getByLabel('Title').fill('Site visit');
			await page.getByLabel('Text').fill('Two floors, 30 desks, old cabling, no labels.');
			await page.getByRole('button', { name: 'Add to discovery' }).click();
			await expect(page.getByRole('list', { name: 'Discovery' })).toContainText('Site visit');

			await page.getByRole('button', { name: 'Draft with AI' }).click();
			await expect(page.getByLabel('In scope', { exact: true })).toHaveValue(
				'- Rewire the 30 desks on both floors',
				{ timeout: 45_000 }
			);
			await expect(page.getByText('AI draft').first()).toBeVisible();
			await expect(page.getByRole('list', { name: 'Open questions' })).toContainText(
				'Which weekend can the office close?'
			);

			await page.getByRole('button', { name: 'Approve' }).click();
			await expect(page.getByText('Approved v1')).toBeVisible();

			// The quote carries it.
			const quote = await ok<{ id: string }>(
				api.post('/api/v1/quotes', {
					data: {
						orgId: client.id,
						quote: {
							title: `Rewire ${stamp}`,
							opportunityId: opp.id,
							lines: [{ name: 'Cabling', quantity: 30, unitPrice: '120' }]
						}
					}
				}),
				201
			);
			await page.goto(`/sales/quotes/${quote.id}`);
			await page.getByLabel('Carries').selectOption({ label: 'Version 1' });
			await page.getByRole('button', { name: 'Save' }).click();
			await expect(
				page.getByRole('region', { name: 'Scope of work' }).or(page.getByLabel('Scope of work'))
			).toContainText('Rewire the 30 desks');
		} finally {
			ai.close();
			await api.dispose();
		}
	});
});
