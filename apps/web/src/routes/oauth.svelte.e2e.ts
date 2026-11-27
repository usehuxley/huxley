import { createHash, randomBytes } from 'node:crypto';
import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: an MCP client signs in with OAuth through the consent page.
// Needs the API with a bootstrapped owner, HUXLEY_API_URL, and
// HUXLEY_AUTH__PUBLIC_URL set to this web app's origin.
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;

// The client's loopback redirect; the test catches the browser there.
const REDIRECT = 'http://127.0.0.1:47999/callback';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

/** Registers a client and returns the authorize URL with fresh PKCE. */
async function startSignIn(page: Page, name: string) {
	const api = await request.newContext({ baseURL: apiUrl });
	const registered = await api.post('/oauth/register', {
		data: { client_name: name, redirect_uris: [REDIRECT] }
	});
	expect(registered.status()).toBe(201);
	const { client_id: clientId } = (await registered.json()) as { client_id: string };
	const verifier = randomBytes(32).toString('base64url');
	const challenge = createHash('sha256').update(verifier).digest('base64url');
	const authorize = new URLSearchParams({
		response_type: 'code',
		client_id: clientId,
		redirect_uri: REDIRECT,
		code_challenge: challenge,
		code_challenge_method: 'S256',
		state: 'e2e-state'
	});
	// Where the browser lands afterwards: caught, as the app would.
	const landed = new Promise<URL>((resolve) => {
		void page.route(`${REDIRECT}**`, async (route) => {
			resolve(new URL(route.request().url()));
			await route.fulfill({ body: 'You can close this window.' });
		});
	});
	return { api, clientId, verifier, url: `/oauth/authorize?${authorize}`, landed };
}

async function mcpStatus(token: string): Promise<number> {
	const api = await request.newContext({ baseURL: apiUrl });
	const res = await api.post('/mcp', {
		headers: { authorization: `Bearer ${token}` },
		data: {
			jsonrpc: '2.0',
			id: 1,
			method: 'tools/call',
			params: { name: 'list_organizations', arguments: {} }
		}
	});
	await api.dispose();
	return res.status();
}

test.describe('OAuth for MCP clients', () => {
	test.skip(!email || !password || !apiUrl, 'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL');

	test('a person connects an app, it uses MCP, and they disconnect it', async ({ page }) => {
		await signIn(page);
		const name = `Agent ${Date.now().toString(36)}`;
		const flow = await startSignIn(page, name);

		await page.goto(flow.url);
		await expect(page.getByRole('heading', { name: `Connect ${name}?` })).toBeVisible();
		await expect(page.getByText('anything your roles allow')).toBeVisible();
		await expect(page.getByText('127.0.0.1', { exact: true })).toBeVisible();
		await page.getByRole('button', { name: 'Connect' }).click();

		const back = await flow.landed;
		expect(back.searchParams.get('state')).toBe('e2e-state');
		const code = back.searchParams.get('code');
		expect(code).toMatch(/^hxa_/);

		const tokens = await flow.api.post('/oauth/token', {
			form: {
				grant_type: 'authorization_code',
				code: code!,
				redirect_uri: REDIRECT,
				code_verifier: flow.verifier,
				client_id: flow.clientId
			}
		});
		expect(tokens.status()).toBe(200);
		const { access_token: access } = (await tokens.json()) as { access_token: string };
		expect(await mcpStatus(access)).toBe(200);

		await page.goto('/profile');
		const apps = page.getByRole('list', { name: 'Connected apps' });
		const row = apps.getByRole('listitem').filter({ hasText: name });
		await expect(row).toContainText('Full access');
		await row.getByRole('button', { name: 'Disconnect' }).click();
		await expect(apps.getByText(name)).toHaveCount(0);
		expect(await mcpStatus(access)).toBe(401);
		await flow.api.dispose();
	});

	test('cancelling sends the app an access_denied error', async ({ page }) => {
		await signIn(page);
		const flow = await startSignIn(page, `Declined ${Date.now().toString(36)}`);
		await page.goto(flow.url);
		await page.getByRole('button', { name: 'Cancel' }).click();
		const back = await flow.landed;
		expect(back.searchParams.get('error')).toBe('access_denied');
		expect(back.searchParams.get('state')).toBe('e2e-state');
		expect(back.searchParams.has('code')).toBe(false);
		await flow.api.dispose();
	});
});
