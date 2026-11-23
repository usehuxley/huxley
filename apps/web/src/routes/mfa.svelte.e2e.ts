import { createHmac } from 'node:crypto';
import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner (see page.svelte.e2e.ts)
// and HUXLEY_API_URL so the test can create its own user directly.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const PASSWORD = 'mfa e2e test passphrase';

function base32Decode(input: string): Buffer {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
	let bits = 0;
	let value = 0;
	const out: number[] = [];
	for (const char of input.replace(/\s/g, '')) {
		value = (value << 5) | alphabet.indexOf(char);
		bits += 5;
		if (bits >= 8) {
			bits -= 8;
			out.push((value >>> bits) & 0xff);
		}
	}
	return Buffer.from(out);
}

/** RFC 6238 code for the current 30-second step plus `offset` steps. */
function totp(secret: Buffer, offset = 0): string {
	const counter = Buffer.alloc(8);
	counter.writeBigInt64BE(BigInt(Math.floor(Date.now() / 30000) + offset));
	const digest = createHmac('sha1', secret).update(counter).digest();
	const start = digest[digest.length - 1] & 0x0f;
	const binary = digest.readUInt32BE(start) & 0x7fffffff;
	return String(binary % 1_000_000).padStart(6, '0');
}

/** Creates a fresh user through the API as the bootstrap owner. */
async function createUser(): Promise<string> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	const login = await api.post('/api/v1/auth/login', {
		data: { email: ownerEmail, password: ownerPassword }
	});
	expect(login.ok()).toBe(true);
	const email = `mfa-${Date.now().toString(36)}@example.com`;
	const created = await api.post('/api/v1/users', {
		data: { email, displayName: 'Morgan Mfa', password: PASSWORD }
	});
	expect(created.status()).toBe(201);
	await api.dispose();
	return email;
}

async function submitPassword(page: Page, email: string) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(PASSWORD);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
}

test.describe('two-factor authentication', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL with a running API'
	);

	test('a user sets up an authenticator and signs in with a code, then a recovery code', async ({
		page
	}) => {
		const email = await createUser();

		// Set up the authenticator from the profile page.
		await submitPassword(page, email);
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
		await page.goto('/profile');
		await page.getByLabel('Password', { exact: true }).first().fill(PASSWORD);
		await page.getByRole('button', { name: 'Set up authenticator app' }).click();
		const secret = base32Decode(await page.getByTestId('totp-secret').innerText());
		await page.getByLabel('Code').fill(totp(secret));
		await page.getByRole('button', { name: 'Turn on' }).click();
		await expect(page.getByText('Save your recovery codes')).toBeVisible();
		const recoveryCode = (await page.locator('ul.font-mono li').first().innerText()).trim();
		await expect(page.getByText('On', { exact: true })).toBeVisible();

		// Signing in now asks for a code. The setup code can't be replayed, so
		// use the next step's.
		await page.getByRole('button', { name: 'Sign out' }).click();
		await submitPassword(page, email);
		await expect(page.getByLabel('Authentication code')).toBeVisible();
		await page.getByLabel('Authentication code').fill('000000');
		await page.getByRole('button', { name: 'Verify' }).click();
		await expect(page.getByRole('alert')).toHaveText('That code is not valid.');
		await page.getByLabel('Authentication code').fill(totp(secret, 1));
		await page.getByRole('button', { name: 'Verify' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');

		// A recovery code works too.
		await page.getByRole('button', { name: 'Sign out' }).click();
		await submitPassword(page, email);
		await page.getByRole('button', { name: 'Use a recovery code' }).click();
		await page.getByLabel('Recovery code').fill(recoveryCode);
		await page.getByRole('button', { name: 'Verify' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
		await page.goto('/profile');
		await expect(page.getByText('9 recovery codes left')).toBeVisible();
	});
});
