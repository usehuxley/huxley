import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts),
// running its scheduler (which ships audit records), with 127.0.0.1 allowed
// as a destination.
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

test.describe('audit streaming', () => {
	test.skip(!email || !password, 'set E2E_EMAIL and E2E_PASSWORD with a running API');

	let server: Server;
	const received: { headers: Record<string, unknown>; records: Record<string, unknown>[] }[] = [];

	test.beforeAll(async () => {
		server = createServer((req, res) => {
			let body = '';
			req.on('data', (chunk: Buffer) => (body += chunk.toString()));
			req.on('end', () => {
				received.push({
					headers: req.headers,
					records: body
						.split('\n')
						.filter((l) => l.trim() !== '')
						.map((l) => JSON.parse(l) as Record<string, unknown>)
				});
				res.writeHead(200).end();
			});
		});
		await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
	});
	test.afterAll(() => {
		server.close();
	});

	test('a stream sends new audit records to a SIEM, and a broken one shows why', async ({
		page
	}) => {
		test.setTimeout(120_000);
		const stamp = Date.now().toString(36);
		const port = (server.address() as AddressInfo).port;
		await signIn(page);
		await page.goto('/settings/audit-streams');

		await page.getByRole('button', { name: 'New stream' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name').fill(`SIEM ${stamp}`);
		await dialog.getByLabel('URL').fill(`http://127.0.0.1:${port}/ingest`);
		await dialog.getByLabel('Header', { exact: true }).fill('X-Api-Key');
		await dialog.getByLabel('Header value').fill(`key-${stamp}`);
		await dialog.getByRole('button', { name: 'Add stream' }).click();
		await expect(dialog).toBeHidden();
		const streams = page.getByRole('list', { name: 'Audit streams' });
		const row = streams.getByRole('listitem').filter({ hasText: `SIEM ${stamp}` });
		await expect(row).toContainText('Webhook');

		// Adding the stream is itself audited, so it's the first thing sent.
		await expect
			.poll(
				() =>
					received.some((r) =>
						r.records.some(
							(rec) =>
								rec.action === 'audit_stream.created' &&
								(rec.details as { name?: string }).name === `SIEM ${stamp}`
						)
					),
				{ timeout: 60_000 }
			)
			.toBe(true);
		const delivery = received.find((r) =>
			r.records.some((rec) => rec.action === 'audit_stream.created')
		)!;
		expect(delivery.headers['x-api-key']).toBe(`key-${stamp}`);
		expect(String(delivery.records[0].hash)).toMatch(/^[0-9a-f]{64}$/);
		await page.reload();
		await expect(row).toContainText('Up to date');

		// The test button reaches the destination too.
		const before = received.length;
		await row.getByRole('button', { name: 'Send a test' }).click();
		await expect(page.getByText('The test event was delivered.')).toBeVisible();
		expect(received.length).toBeGreaterThan(before);

		// Pointed somewhere that refuses, it says why.
		await row.getByRole('button', { name: `SIEM ${stamp}` }).click();
		await dialog.getByLabel('URL').fill('http://127.0.0.1:9/ingest');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(dialog).toBeHidden();
		await row.getByRole('button', { name: 'Send a test' }).click();
		await expect(page.getByText(/error sending request|connect/i).first()).toBeVisible();

		// Removed, it's gone.
		await row.getByRole('button', { name: `SIEM ${stamp}` }).click();
		await dialog.getByRole('button', { name: 'Remove' }).click();
		await expect(row).toHaveCount(0);
	});
});
