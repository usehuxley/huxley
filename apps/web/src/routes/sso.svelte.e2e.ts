import { createHash, generateKeyPairSync, randomUUID, sign } from 'node:crypto';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner, HUXLEY_API_URL, and
// HUXLEY_AUTH__PUBLIC_URL=http://localhost:4173 (the SSO redirect address).
// A mock OpenID Connect provider runs in this process on 127.0.0.1.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const CLIENT_ID = 'huxley-e2e';
const CLIENT_SECRET = 'e2e-client-secret';
const DOMAIN = 'sso.example.com';

const b64url = (data: Buffer | string) => Buffer.from(data).toString('base64url');

/** A minimal OIDC provider that approves everyone as `user`. */
function startMockIdp(user: { sub: string; email: string; groups: string[] }) {
	const { privateKey, publicKey } = generateKeyPairSync('rsa', { modulusLength: 2048 });
	const jwk = { ...publicKey.export({ format: 'jwk' }), kid: 'e2e', alg: 'RS256', use: 'sig' };
	const codes = new Map<string, { nonce: string; challenge: string }>();
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
			// The user "approves" immediately.
			const code = randomUUID();
			codes.set(code, {
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
			req.on('data', (chunk: Buffer) => (body += chunk.toString()));
			req.on('end', () => {
				const form = new URLSearchParams(body);
				const basic = `Basic ${Buffer.from(`${CLIENT_ID}:${CLIENT_SECRET}`).toString('base64')}`;
				if (req.headers.authorization !== basic) return json(401, { error: 'invalid_client' });
				const pending = codes.get(form.get('code') ?? '');
				codes.delete(form.get('code') ?? '');
				const verifier = form.get('code_verifier') ?? '';
				if (
					!pending ||
					b64url(createHash('sha256').update(verifier).digest()) !== pending.challenge
				) {
					return json(400, { error: 'invalid_grant' });
				}
				const now = Math.floor(Date.now() / 1000);
				const header = b64url(JSON.stringify({ alg: 'RS256', kid: 'e2e', typ: 'JWT' }));
				const payload = b64url(
					JSON.stringify({
						iss: issuer,
						aud: CLIENT_ID,
						sub: user.sub,
						email: user.email,
						email_verified: true,
						name: 'Sam Sso',
						groups: user.groups,
						amr: ['pwd', 'mfa'],
						nonce: pending.nonce,
						iat: now,
						exp: now + 300
					})
				);
				const signature = b64url(sign('sha256', Buffer.from(`${header}.${payload}`), privateKey));
				json(200, {
					access_token: 'opaque',
					token_type: 'Bearer',
					expires_in: 300,
					id_token: `${header}.${payload}.${signature}`
				});
			});
			return;
		}
		json(404, { error: 'not_found' });
	});

	return new Promise<{ issuer: string; close: () => void }>((done) => {
		server.listen(0, '127.0.0.1', () => {
			issuer = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
			done({ issuer, close: () => server.close() });
		});
	});
}

test.describe('single sign-on', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL with a running API'
	);

	test('signing in with an OIDC provider creates the user and maps their roles', async ({
		page
	}) => {
		const suffix = Date.now().toString(36);
		const email = `sam-${suffix}@${DOMAIN}`;
		const idp = await startMockIdp({ sub: `sam-${suffix}`, email, groups: ['huxley-readers'] });

		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		expect(
			(
				await api.post('/api/v1/auth/login', {
					data: { email: ownerEmail, password: ownerPassword }
				})
			).ok()
		).toBe(true);
		const roles = (await (await api.get('/api/v1/roles')).json()) as { id: string; key: string }[];
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const created = await api.post('/api/v1/security/sso/providers', {
			data: {
				key: `mock-${suffix}`,
				name: `Mock ${suffix}`,
				preset: 'generic',
				issuerUrl: idp.issuer,
				clientId: CLIENT_ID,
				clientSecret: CLIENT_SECRET,
				allowedDomains: [DOMAIN],
				jitProvisioning: true,
				roleClaim: 'groups'
			}
		});
		expect(created.status()).toBe(201);
		const provider = (await created.json()) as { id: string };
		const mapped = await api.put(`/api/v1/security/sso/providers/${provider.id}/role-mappings`, {
			data: [
				{
					claimValue: 'huxley-readers',
					roleId: roles.find((r) => r.key === 'read_only')!.id,
					orgId: orgs.find((o) => o.parentId === null)!.id,
					inherit: true
				}
			]
		});
		expect(mapped.status()).toBe(204);

		try {
			// Start from a page that needs Users access: the mapped Read-only
			// role grants it, and sign-in returns there.
			await page.goto('/settings/users');
			await expect(page).toHaveURL(/\/login\?next=%2Fsettings%2Fusers$/);
			await page.getByRole('link', { name: `Sign in with Mock ${suffix}` }).click();
			await expect(page).toHaveURL(/\/settings\/users$/);
			await expect(page.getByText(email)).toBeVisible();
		} finally {
			await api.delete(`/api/v1/security/sso/providers/${provider.id}`);
			await api.dispose();
			idp.close();
		}
	});

	test('a callback that did not start in this browser is refused', async ({ page }) => {
		await page.goto('/login/oidc/callback?code=x&state=hxo_forged');
		await expect(page).toHaveURL(/\/login\?sso_error=failed$/);
		await expect(page.getByRole('alert')).toHaveText(
			"Single sign-on couldn't be completed. Try again."
		);
	});
});
