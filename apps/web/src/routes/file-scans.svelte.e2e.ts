import { createServer, type Server, type Socket } from 'node:net';
import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: the API runs with uploads.scan set to ClamAV at
// 127.0.0.1:13399 and while_pending = "allow" (see the e2e job). This test
// is that ClamAV: a fake clamd that finds the EICAR test file.
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const CLAMD_PORT = 13399;

/** The EICAR test file, put together at runtime so this source isn't flagged. */
const EICAR = Buffer.from(
	['X5O!P%@AP[4\\PZX54(P^)7CC)7}$', 'EICAR-STANDARD-ANTIVIRUS-TEST-FILE!$H+H*'].join('')
);

/** Answers clamd's zVERSION and zINSTREAM. */
function serve(socket: Socket) {
	let buffer = Buffer.alloc(0);
	let command: string | null = null;
	const data: Buffer[] = [];
	socket.on('data', (chunk: Buffer) => {
		buffer = Buffer.concat([buffer, chunk]);
		if (command === null) {
			const end = buffer.indexOf(0);
			if (end < 0) return;
			command = buffer.subarray(0, end).toString();
			buffer = buffer.subarray(end + 1);
			if (command === 'zVERSION') {
				socket.end('ClamAV 1.4.7/e2e\0');
				return;
			}
		}
		while (buffer.length >= 4) {
			const length = buffer.readUInt32BE(0);
			if (length === 0) {
				const all = Buffer.concat(data);
				socket.end(all.includes(EICAR) ? 'stream: Eicar-Signature FOUND\0' : 'stream: OK\0');
				return;
			}
			if (buffer.length < 4 + length) return;
			data.push(buffer.subarray(4, 4 + length));
			buffer = buffer.subarray(4 + length);
		}
	});
	socket.on('error', () => {});
}

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

test.describe('malware scanning', () => {
	test.skip(!email || !password || !apiUrl, 'set E2E_EMAIL, E2E_PASSWORD and HUXLEY_API_URL');

	let clamd: Server;
	test.beforeAll(async () => {
		clamd = createServer(serve);
		await new Promise<void>((done) => clamd.listen(CLAMD_PORT, '127.0.0.1', done));
	});
	test.afterAll(async () => {
		await new Promise((done) => clamd.close(done));
	});

	test('an infected upload is quarantined, listed and deleted', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email, password } });
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const filename = `invoice-${stamp}.pdf.exe`;
		const uploaded = await api.post(`/api/v1/entities/organization/${root}/attachments`, {
			multipart: { file: { name: filename, mimeType: 'application/octet-stream', buffer: EICAR } }
		});
		expect(uploaded.status(), await uploaded.text()).toBe(201);
		const { id } = (await uploaded.json()) as { id: string };
		// The scheduler scans every couple of seconds.
		await expect(async () => {
			const files = (await (
				await api.get(`/api/v1/entities/organization/${root}/attachments`)
			).json()) as { id: string; scanStatus: string }[];
			expect(files.find((f) => f.id === id)?.scanStatus).toBe('infected');
		}).toPass({ timeout: 30_000 });
		await api.dispose();

		await signIn(page);
		// Downloading says why it can't.
		const download = await page.request.get(`/attachments/${id}`);
		expect(download.status()).toBe(403);
		expect(await download.text()).toContain('found a threat');

		// The organization's files show it quarantined, without a link.
		await page.goto(`/settings/organizations/${root}`);
		const row = page.getByRole('listitem').filter({ hasText: filename });
		await expect(row.getByText('Quarantined')).toBeVisible();
		await expect(row.getByRole('link', { name: filename })).toHaveCount(0);

		// Administrators see it, with the finding, and delete it.
		await page.goto('/settings/file-scans');
		const scanner = page.getByLabel('Scanner');
		await expect(scanner).toContainText('Answering');
		await expect(scanner).toContainText('ClamAV 1.4.7/e2e');
		const listed = page.getByRole('list', { name: 'Scanned files' }).getByRole('listitem').filter({
			hasText: filename
		});
		await expect(listed).toContainText('Found: Eicar-Signature');
		await listed.getByRole('button', { name: `Delete ${filename}` }).click();
		await expect(page.getByText('File deleted.')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Scanned files' }).getByText(filename)).toHaveCount(
			0
		);
	});
});
