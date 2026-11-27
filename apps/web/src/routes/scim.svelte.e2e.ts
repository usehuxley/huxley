import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
// Turns SCIM on for a provider in the UI, then provisions a person with the
// token the way an identity provider would.
const email = process.env.E2E_EMAIL;
const password = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(email!);
	await page.getByLabel('Password').fill(password!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

test.describe('SCIM provisioning', () => {
	test.skip(!email || !password || !apiUrl, 'set E2E_EMAIL, E2E_PASSWORD and HUXLEY_API_URL');

	test('an identity provider provisions a person, who then shows in Huxley', async ({ page }) => {
		const suffix = Date.now().toString(36);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		expect((await api.post('/api/v1/auth/login', { data: { email, password } })).ok()).toBe(true);
		const created = await api.post('/api/v1/security/sso/providers', {
			data: {
				key: `entra-${suffix}`,
				name: `Entra ${suffix}`,
				preset: 'generic',
				issuerUrl: 'https://login.example.com',
				clientId: 'huxley',
				clientSecret: 'secret',
				allowedDomains: ['contoso.example']
			}
		});
		expect(created.status(), await created.text()).toBe(201);
		const provider = (await created.json()) as { id: string };
		await api.dispose();

		await signIn(page);
		await page.goto(`/settings/sso/${provider.id}`);
		await page.getByRole('button', { name: 'Turn on provisioning' }).click();
		const token = (await page.getByTestId('scim-token').textContent())!.trim();
		expect(token).toMatch(/^hxscim_/);
		await expect(page.getByTestId('scim-status')).toContainText('0 people and 0 teams provisioned');

		// The identity provider, with the token.
		const idp = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: {
				authorization: `Bearer ${token}`,
				'content-type': 'application/scim+json'
			}
		});
		const userName = `grace.${suffix}@contoso.example`;
		const made = await idp.post('/scim/v2/Users', {
			data: {
				schemas: ['urn:ietf:params:scim:schemas:core:2.0:User'],
				userName,
				displayName: `Grace ${suffix}`,
				active: true
			}
		});
		expect(made.status(), await made.text()).toBe(201);
		const group = await idp.post('/scim/v2/Groups', {
			data: {
				schemas: ['urn:ietf:params:scim:schemas:core:2.0:Group'],
				displayName: `Engineering ${suffix}`,
				members: [{ value: ((await made.json()) as { id: string }).id }]
			}
		});
		expect(group.status(), await group.text()).toBe(201);
		await idp.dispose();

		// They're a Huxley user now, in the synced team.
		await page.reload();
		await expect(page.getByTestId('scim-status')).toContainText('1 person and 1 team provisioned');
		await page.goto('/settings/users');
		await expect(page.getByText(`Grace ${suffix}`)).toBeVisible();
		await page.goto('/settings/teams');
		await expect(page.getByText(`Engineering ${suffix}`)).toBeVisible();

		// Turned off, the token stops working.
		await page.goto(`/settings/sso/${provider.id}`);
		await page.getByRole('button', { name: 'Turn off' }).click();
		await expect(page.getByText('Provisioning is off.')).toBeVisible();
		const refused = await (
			await request.newContext({ baseURL: apiUrl })
		).get('/scim/v2/Users', { headers: { authorization: `Bearer ${token}` } });
		expect(refused.status()).toBe(401);
	});
});
