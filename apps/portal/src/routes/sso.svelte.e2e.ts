import { createHash, generateKeyPairSync, randomUUID, sign } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test } from '@playwright/test';

// Full-stack: a contact signs in with their company's own identity provider.
// The provider is a mock OIDC server in this test process (RS256 ID tokens,
// PKCE and the client secret checked), standing in for the client's Entra
// tenant. Needs the service-desk preset and the API's portal URL set to this
// preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

const CLIENT_ID = 'acme-portal';
const CLIENT_SECRET = 'acme-tenant-secret';
const b64url = (data: Buffer | string) => Buffer.from(data).toString('base64url');

/** A tiny OIDC provider: discovery, keys, an authorize step that approves at once, tokens. */
function mockProvider(person: { sub: string; email: string; name: string }) {
	const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
	const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'e2e', use: 'sig', alg: 'RS256' };
	const pending = new Map<string, { nonce: string; challenge: string }>();
	let issuer = '';
	const server: Server = createServer((req, res) => {
		const url = new URL(req.url ?? '/', issuer);
		const json = (status: number, body: unknown) => {
			res.writeHead(status, { 'content-type': 'application/json' });
			res.end(JSON.stringify(body));
		};
		if (url.pathname === '/.well-known/openid-configuration') {
			return json(200, {
				issuer,
				authorization_endpoint: `${issuer}/authorize`,
				token_endpoint: `${issuer}/token`,
				jwks_uri: `${issuer}/jwks`,
				response_types_supported: ['code'],
				subject_types_supported: ['public'],
				id_token_signing_alg_values_supported: ['RS256']
			});
		}
		if (url.pathname === '/jwks') return json(200, { keys: [jwk] });
		if (url.pathname === '/authorize') {
			const code = randomUUID();
			pending.set(code, {
				nonce: url.searchParams.get('nonce') ?? '',
				challenge: url.searchParams.get('code_challenge') ?? ''
			});
			const back = new URL(url.searchParams.get('redirect_uri') ?? '');
			back.searchParams.set('code', code);
			back.searchParams.set('state', url.searchParams.get('state') ?? '');
			res.writeHead(302, { location: back.toString() });
			return res.end();
		}
		if (url.pathname === '/token' && req.method === 'POST') {
			let body = '';
			req.on('data', (c: Buffer) => (body += c.toString()));
			req.on('end', () => {
				const form = new URLSearchParams(body);
				const basic = `Basic ${Buffer.from(`${CLIENT_ID}:${CLIENT_SECRET}`).toString('base64')}`;
				if (req.headers.authorization !== basic) return json(401, { error: 'invalid_client' });
				const p = pending.get(form.get('code') ?? '');
				const verifier = form.get('code_verifier') ?? '';
				if (!p || b64url(createHash('sha256').update(verifier).digest()) !== p.challenge) {
					return json(400, { error: 'invalid_grant' });
				}
				pending.delete(form.get('code') ?? '');
				const now = Math.floor(Date.now() / 1000);
				const header = b64url(JSON.stringify({ alg: 'RS256', typ: 'JWT', kid: 'e2e' }));
				const claims = b64url(
					JSON.stringify({
						iss: issuer,
						aud: CLIENT_ID,
						iat: now,
						exp: now + 300,
						nonce: p.nonce,
						...person
					})
				);
				const signature = b64url(sign('sha256', Buffer.from(`${header}.${claims}`), privateKey));
				json(200, {
					access_token: 'opaque',
					token_type: 'Bearer',
					expires_in: 300,
					id_token: `${header}.${claims}.${signature}`
				});
			});
			return;
		}
		json(404, {});
	});
	return {
		start: () =>
			new Promise<string>((ok) =>
				server.listen(0, '127.0.0.1', () => {
					issuer = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
					ok(issuer);
				})
			),
		stop: () => new Promise((ok) => server.close(ok))
	};
}

test.describe('portal sign-in with the client’s identity provider', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a contact signs in with their company’s account', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const domain = `acme-${stamp}.example`;
		const provider = mockProvider({ sub: `entra-${stamp}`, email: `ada@${domain}`, name: 'Ada' });
		const issuer = await provider.start();
		try {
			const api = await request.newContext({
				baseURL: apiUrl,
				extraHTTPHeaders: { 'x-requested-with': 'huxley' }
			});
			await api.post('/api/v1/auth/login', {
				data: { email: ownerEmail, password: ownerPassword }
			});
			const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
				id: string;
				parentId: string | null;
			}[];
			const client = await api.post('/api/v1/orgs', {
				data: {
					parentId: orgs.find((o) => o.parentId === null)!.id,
					kind: 'client',
					name: `Acme ${stamp}`
				}
			});
			const { id: clientId } = (await client.json()) as { id: string };
			await api.post(`/api/v1/orgs/${clientId}/contacts`, {
				data: { firstName: 'Ada', lastName: stamp, email: `ada@${domain}` }
			});
			// The client's tenant, giving its contacts access on first sign-in.
			const created = await api.post(`/api/v1/orgs/${clientId}/portal-identity-providers`, {
				data: {
					name: 'Acme (Microsoft)',
					preset: 'generic',
					issuerUrl: issuer,
					clientId: CLIENT_ID,
					clientSecret: CLIENT_SECRET,
					emailDomains: [domain],
					provision: true
				}
			});
			expect(created.status(), await created.text()).toBe(201);
			await api.dispose();

			await page.goto('/login');
			await page.getByLabel('Work email').fill(`ada@${domain}`);
			await page.getByRole('button', { name: 'Continue' }).click();
			// Off to the provider and straight back, signed in.
			await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Ada?');
			await expect(page.getByText(`Acme ${stamp}`).first()).toBeVisible();
		} finally {
			await provider.stop();
		}
	});
});
