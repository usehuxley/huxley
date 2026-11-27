import { createHash } from 'node:crypto';
import { expect, test, type Browser, type Page } from '@playwright/test';

const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;

async function signIn(browser: Browser): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
	return page;
}

const CRC_TABLE = Array.from({ length: 256 }, (_, n) => {
	let c = n;
	for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
	return c >>> 0;
});

function crc32(data: Buffer): number {
	let c = 0xffffffff;
	for (const byte of data) c = CRC_TABLE[(c ^ byte) & 0xff] ^ (c >>> 8);
	return (c ^ 0xffffffff) >>> 0;
}

/** A zip of uncompressed entries: enough for a test pack. */
function zip(entries: [string, string][]): Buffer {
	const local: Buffer[] = [];
	const central: Buffer[] = [];
	let offset = 0;
	for (const [name, text] of entries) {
		const data = Buffer.from(text);
		const fileName = Buffer.from(name);
		const crc = crc32(data);
		const header = Buffer.alloc(30);
		header.writeUInt32LE(0x04034b50, 0);
		header.writeUInt16LE(20, 4);
		header.writeUInt32LE(crc, 14);
		header.writeUInt32LE(data.length, 18);
		header.writeUInt32LE(data.length, 22);
		header.writeUInt16LE(fileName.length, 26);
		local.push(header, fileName, data);
		const entry = Buffer.alloc(46);
		entry.writeUInt32LE(0x02014b50, 0);
		entry.writeUInt16LE(20, 4);
		entry.writeUInt16LE(20, 6);
		entry.writeUInt32LE(crc, 16);
		entry.writeUInt32LE(data.length, 20);
		entry.writeUInt32LE(data.length, 24);
		entry.writeUInt16LE(fileName.length, 28);
		entry.writeUInt32LE(offset, 42);
		central.push(entry, fileName);
		offset += header.length + fileName.length + data.length;
	}
	const directory = Buffer.concat(central);
	const end = Buffer.alloc(22);
	end.writeUInt32LE(0x06054b50, 0);
	end.writeUInt16LE(entries.length, 8);
	end.writeUInt16LE(entries.length, 10);
	end.writeUInt32LE(directory.length, 12);
	end.writeUInt32LE(offset, 16);
	return Buffer.concat([...local, directory, end]);
}

/** An unsigned pack with one workflow that calls hooks.example.net. */
function pack(suffix: string, tamper = false): Buffer {
	const workflow = JSON.stringify({
		apiVersion: 'usehuxley.io/v1',
		kind: 'Workflow',
		metadata: { key: `ping-${suffix}`, name: `Ping ${suffix}` },
		spec: {
			steps: [
				{
					id: 'ping',
					type: 'http.request',
					with: { method: 'GET', url: 'https://hooks.example.net/ping' }
				}
			]
		}
	});
	const path = `workflows/ping-${suffix}.yaml`;
	const manifest = JSON.stringify({
		apiVersion: 'usehuxley.io/v1',
		kind: 'Pack',
		key: `e2e/ping-${suffix}`,
		name: `Ping pack ${suffix}`,
		version: '1.0.0',
		publisher: 'E2E',
		contents: { workflows: [`ping-${suffix}`] },
		files: { [path]: createHash('sha256').update(workflow).digest('hex') }
	});
	// A tampered pack's workflow changed after the manifest was made.
	const shipped = tamper ? workflow.replace('hooks.example.net', 'evil.example.org') : workflow;
	return zip([
		['manifest.json', manifest],
		[path, shipped]
	]);
}

test.describe('packs', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('a pack is reviewed, then installed as drafts', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		await page.goto('/settings/packs');
		await page.getByLabel('Pack file').setInputFiles({
			name: 'ping.hxpack',
			mimeType: 'application/octet-stream',
			buffer: pack(suffix)
		});
		await page.getByRole('button', { name: 'Review', exact: true }).click();

		const review = page.getByTestId('pack-review');
		await expect(review).toBeVisible();
		await expect(review.getByText('Unsigned', { exact: true })).toBeVisible();
		await expect(page.getByRole('alert').first()).toContainText('Nobody vouches');
		await expect(page.getByRole('list', { name: 'Pack contents' })).toContainText(`ping-${suffix}`);
		await expect(page.getByRole('list', { name: 'Egress domains' })).toHaveText(
			'hooks.example.net'
		);

		await page.getByRole('button', { name: 'Install' }).click();
		await expect(page.getByText(`Installed e2e/ping-${suffix} 1.0.0`)).toBeVisible();
		await expect(page.getByRole('list', { name: 'Installed packs' })).toContainText(
			`Ping pack ${suffix} 1.0.0`
		);
	});

	test('items are exported as a pack', async ({ browser }) => {
		const page = await signIn(browser);
		const suffix = Date.now().toString(36);
		// Something to export: install a pack's workflow first.
		await page.goto('/settings/packs');
		await page.getByLabel('Pack file').setInputFiles({
			name: 'ping.hxpack',
			mimeType: 'application/octet-stream',
			buffer: pack(suffix)
		});
		await page.getByRole('button', { name: 'Review', exact: true }).click();
		await page.getByRole('button', { name: 'Install' }).click();
		await expect(page.getByText(`Installed e2e/ping-${suffix} 1.0.0`)).toBeVisible();

		await page.getByLabel('Pack key').fill(`e2e/export-${suffix}`);
		await page.getByLabel('Pack name').fill('Exported');
		await page.getByRole('checkbox', { name: `Ping ${suffix}` }).check();
		const download = page.waitForEvent('download');
		await page.getByRole('button', { name: 'Download pack' }).click();
		const file = await download;
		expect(file.suggestedFilename()).toBe(`e2e-export-${suffix}-1.0.0.hxpack`);
		const stream = await file.createReadStream();
		const chunks: Buffer[] = [];
		for await (const chunk of stream) chunks.push(chunk as Buffer);
		const bytes = Buffer.concat(chunks);
		// A zip, whose manifest names the workflow.
		expect(bytes.subarray(0, 2).toString()).toBe('PK');
		expect(bytes.includes(`workflows/ping-${suffix}.yaml`)).toBe(true);
	});

	test('built-in packs are trusted by construction', async ({ browser }) => {
		const page = await signIn(browser);
		await page.goto('/settings/packs');
		await page.getByRole('button', { name: 'Review Slack' }).click();
		const review = page.getByTestId('pack-review');
		await expect(review.getByText('Built in', { exact: true })).toBeVisible();
		await expect(page.getByRole('list', { name: 'Egress domains' })).toHaveText('slack.com');
		await expect(review.getByText('Nobody vouches')).toHaveCount(0);
	});

	test('a damaged pack is refused', async ({ browser }) => {
		const page = await signIn(browser);
		await page.goto('/settings/packs');
		const bytes = pack(Date.now().toString(36), true);
		await page.getByLabel('Pack file').setInputFiles({
			name: 'bad.hxpack',
			mimeType: 'application/octet-stream',
			buffer: bytes
		});
		await page.getByRole('button', { name: 'Review', exact: true }).click();
		await expect(page.getByText("doesn't match the manifest")).toBeVisible();
		await expect(page.getByTestId('pack-review')).toHaveCount(0);
	});
});
