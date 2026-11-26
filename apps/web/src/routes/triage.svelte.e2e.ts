import { createServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack (ADR-047): at a client with two queues, a stand-in AI provider
// and the built-in AI triage pack, an accuracy evaluation runs the pack's
// workflow on each labeled sample and scores it against the target. The
// stand-in sends the guest Wi-Fi sample to the wrong queue: 3 of 4.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** Answers like Anthropic's Messages API, picking from the schema's queues by keyword. */
async function model(): Promise<{ port: number; close: () => void }> {
	const server = createServer((req, res) => {
		let body = '';
		req.on('data', (chunk) => (body += chunk));
		req.on('end', () => {
			const parsed = JSON.parse(body) as {
				model: string;
				messages: unknown[];
				output_config: { format: { schema: { properties: { queue: { enum: string[] } } } } };
			};
			const queues = parsed.output_config.format.schema.properties.queue.enum;
			const text = JSON.stringify(parsed.messages).toLowerCase();
			const pick = (word: string) => queues.find((q) => q.startsWith(word))!;
			const answer = text.includes('offline')
				? { queue: pick('Network'), priority: 'p1', skills: [], reason: 'Everyone is offline' }
				: text.includes('monitor')
					? { queue: pick('Hardware'), priority: 'p3', skills: [], reason: 'A hardware request' }
					: { queue: pick('Hardware'), priority: 'p2', skills: [], reason: 'Sounds like a device' };
			res.writeHead(200, { 'content-type': 'application/json' });
			res.end(
				JSON.stringify({
					model: parsed.model,
					stop_reason: 'end_turn',
					content: [{ type: 'text', text: JSON.stringify(answer) }],
					usage: { input_tokens: 30, output_tokens: 8 }
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

async function ok(response: Promise<import('@playwright/test').APIResponse>, status = 201) {
	const r = await response;
	expect(r.status(), await r.text()).toBe(status);
	return (await r.json()) as unknown;
}

/** A client with Hardware and Network queues, the stand-in provider and the pack: its id and queues. */
async function setUp(api: APIRequestContext, stamp: string, port: number) {
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = (await ok(
		api.post('/api/v1/orgs', {
			data: {
				parentId: orgs.find((o) => o.parentId === null)!.id,
				kind: 'client',
				name: `Triage ${stamp}`
			}
		})
	)) as { id: string };
	const queue = async (name: string, description: string) =>
		(await ok(
			api.post('/api/v1/queues', {
				data: { orgId: client.id, name: `${name} ${stamp}`, description }
			})
		)) as { id: string; name: string };
	const hardware = await queue('Hardware', 'Laptops, monitors, printers');
	const network = await queue('Network', 'Internet, Wi-Fi, VPN');
	await ok(
		api.put('/api/v1/ai/provider', {
			data: {
				orgId: client.id,
				use: 'chat',
				provider: { kind: 'anthropic', baseUrl: `http://127.0.0.1:${port}`, apiKey: 'sk-ant-e2e' }
			}
		}),
		200
	);
	const review = (await ok(
		api.post(`/api/v1/packs/builtin/huxley/ai-triage?orgId=${client.id}`)
	)) as {
		uploadId: string;
		version: string;
	};
	const installed = (await ok(
		api.post(`/api/v1/packs/uploads/${review.uploadId}/install`, {
			data: { orgId: client.id, version: review.version }
		})
	)) as { items: { workflows: Record<string, string> } };
	for (const id of Object.values(installed.items.workflows)) {
		await ok(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }), 200);
	}
	return { clientId: client.id, hardware, network };
}

test.describe('AI triage', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('accuracy is measured on labeled samples', async ({ page }) => {
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
			const { clientId, hardware, network } = await setUp(api, stamp, ai.port);
			for (const [text, queueId, priority] of [
				['My laptop will not start', hardware.id, 'p2'],
				['The whole office is offline', network.id, 'p1'],
				['Can visitors get guest Wi-Fi?', network.id, 'p4']
			]) {
				await ok(
					api.post('/api/v1/triage/samples', { data: { orgId: clientId, text, queueId, priority } })
				);
			}

			await page.goto('/login');
			await page.getByLabel('Email').fill(ownerEmail!);
			await page.getByLabel('Password').fill(ownerPassword!);
			await page.getByRole('button', { name: 'Sign in', exact: true }).click();
			await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
			await page
				.context()
				.addCookies([{ name: 'hx_org', value: clientId, url: new URL(page.url()).origin }]);
			await page.goto('/settings/triage');

			// One more sample from the page.
			await page.getByLabel('Ticket text').fill('I need a second monitor');
			await page.getByLabel('Right queue').selectOption({ label: hardware.name });
			await page.getByLabel('Right priority').selectOption({ label: 'P3' });
			await page.getByRole('button', { name: 'Add sample' }).click();
			await expect(page.getByRole('list', { name: 'Samples' }).getByRole('listitem')).toHaveCount(
				4
			);

			await page.getByRole('button', { name: 'Run evaluation' }).click();
			await expect(page.getByText('Running 4 samples through the workflow.')).toBeVisible();
			const latest = page.getByRole('region', { name: 'Latest evaluation' });
			await expect(async () => {
				await page.reload();
				await expect(latest.getByText('Below the target')).toBeVisible({ timeout: 1000 });
			}).toPass({ timeout: 30_000 });
			await expect(latest).toContainText('75%');
			await expect(latest).toContainText('target 80%');
			const misses = page.getByRole('table', { name: 'Misses' });
			await expect(misses.getByRole('row')).toHaveCount(2);
			await expect(misses).toContainText('Can visitors get guest Wi-Fi?');
			await expect(misses).toContainText(`${network.name} · P4`);
			await expect(misses).toContainText(`${hardware.name} · P2`);
		} finally {
			await api.dispose();
			ai.close();
		}
	});
});
