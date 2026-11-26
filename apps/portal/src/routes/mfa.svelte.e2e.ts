import { createHmac } from 'node:crypto';
import { expect, request, test } from '@playwright/test';

// Full-stack with real email: a client requires two-factor sign-in, so the
// contact sets up an authenticator before anything else, and their next
// sign-in asks for a code. Needs the service-desk preset, MAILPIT_URL and
// the API's portal URL set to this preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** An authenticator app's code (RFC 6238, SHA-1, 6 digits, 30 s). */
function totp(base32: string, stepOffset = 0): string {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
	let bits = '';
	for (const c of base32.replace(/=+$/, ''))
		bits += alphabet.indexOf(c).toString(2).padStart(5, '0');
	const key = Buffer.from(bits.match(/.{8}/g)!.map((b) => parseInt(b, 2)));
	const counter = Buffer.alloc(8);
	counter.writeBigUInt64BE(BigInt(Math.floor(Date.now() / 30000) + stepOffset));
	const mac = createHmac('sha1', key).update(counter).digest();
	const offset = mac[mac.length - 1] & 0xf;
	return ((mac.readUInt32BE(offset) & 0x7fffffff) % 1_000_000).toString().padStart(6, '0');
}

/** The newest portal sign-in link emailed to `to`, other than `not`. */
async function linkIn(to: string, not?: string): Promise<string> {
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
				if (match && match[0] !== not) return match[0];
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no new email to ${to}`);
	} finally {
		await api.dispose();
	}
}

async function staff() {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	return api;
}

/** A client requiring two-factor sign-in, with an invited contact. */
async function invite(stamp: string) {
	const api = await staff();
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `MFA ${stamp}`
		}
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const required = await api.put(`/api/v1/orgs/${clientId}/settings/portal.require_mfa`, {
		data: { value: 'true', locked: false }
	});
	expect(required.status(), await required.text()).toBeLessThan(300);
	const email = `bea-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Bea', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_user' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	const { account } = (await invited.json()) as { account: { id: string } };
	await api.dispose();
	return { email, account: account.id };
}

test.describe('portal two-factor sign-in', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a required authenticator is set up, then asked for', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const { email, account } = await invite(stamp);
		const first = await linkIn(email);
		const link = new URL(first);
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();

		// Required: setting it up comes first, wherever they go.
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Two-factor sign-in');
		await page.goto('/tickets');
		await expect(page).toHaveURL(/\/profile\/security$/);
		await expect(page.getByText('Your organization requires two-factor sign-in.')).toBeVisible();
		await page.getByRole('button', { name: 'Set up an authenticator app' }).click();
		const key = (await page.locator('code').textContent())!.trim();
		await page.getByLabel('Code from the app').fill(totp(key));
		await page.getByRole('button', { name: 'Turn on' }).click();
		await expect(
			page.getByRole('list', { name: 'Recovery codes' }).getByRole('listitem')
		).toHaveCount(10);
		await page.getByRole('link', { name: 'Done' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Bea?');

		// Next time, a code is asked for after the link.
		await page.getByRole('button', { name: 'Sign out' }).click();
		await expect(page.getByLabel('Work email')).toBeVisible();
		// (Links are one a minute when asked for; staff can send one any time.)
		const api = await staff();
		await api.post(`/api/v1/portal-accounts/${account}/invitation`);
		await api.dispose();
		const second = new URL(await linkIn(email, first));
		await page.goto(second.pathname + second.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Enter your code');
		await page.getByLabel('Code').fill('000000');
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByText('That code didn’t work.', { exact: false })).toBeVisible();
		// The code used at setup is spent; the app's next one works.
		await page.getByLabel('Code').fill(totp(key, 1));
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Bea?');
	});
});
