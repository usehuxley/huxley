import { expect, request, test } from '@playwright/test';
import { base32Decode, totp } from '../lib/testing/totp.ts';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const PASSWORD = 'policy e2e passphrase';

/** An API client signed in as the bootstrap owner. */
async function ownerApi() {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	const login = await api.post('/api/v1/auth/login', {
		data: { email: ownerEmail, password: ownerPassword }
	});
	expect(login.ok()).toBe(true);
	return api;
}

test.describe('MFA policy', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL with a running API'
	);

	test('a required role sends its holders to set up two-factor first', async ({ browser }) => {
		// A Read-only user at the root. No other test signs in with this role.
		const api = await ownerApi();
		const email = `policy-${Date.now().toString(36)}@example.com`;
		const created = await api.post('/api/v1/users', {
			data: { email, displayName: 'Robin Readonly', password: PASSWORD }
		});
		const userId = ((await created.json()) as { user: { id: string } }).user.id;
		const roles = (await (await api.get('/api/v1/roles')).json()) as { id: string; key: string }[];
		const readOnly = roles.find((r) => r.key === 'read_only')!;
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!;
		expect(
			(
				await api.post('/api/v1/role-assignments', {
					data: { userId, roleId: readOnly.id, orgId: root.id }
				})
			).status()
		).toBe(201);

		// The owner requires two-factor for Read-only from Settings → Security.
		const ownerPage = await (await browser.newContext()).newPage();
		await ownerPage.goto('/login');
		await ownerPage.getByLabel('Email').fill(ownerEmail!);
		await ownerPage.getByLabel('Password').fill(ownerPassword!);
		await ownerPage.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(ownerPage.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await ownerPage.goto('/settings/security');
		await ownerPage.getByRole('button', { name: 'Require for Read-only' }).click();
		await expect(
			ownerPage.getByRole('button', { name: 'Stop requiring for Read-only' })
		).toBeVisible();

		try {
			// The user signs in and lands on their profile, wherever they meant to go.
			const page = await (await browser.newContext()).newPage();
			await page.goto('/settings/organizations');
			await page.getByLabel('Email').fill(email);
			await page.getByLabel('Password').fill(PASSWORD);
			await page.getByRole('button', { name: 'Sign in', exact: true }).click();
			await expect(page).toHaveURL(/\/profile$/);
			await expect(page.getByTestId('mfa-required')).toBeVisible();
			await page.goto('/');
			await expect(page).toHaveURL(/\/profile$/);

			// Setting up an authenticator app lifts the requirement.
			await page.locator('#totpPassword').fill(PASSWORD);
			await page.getByRole('button', { name: 'Set up authenticator app' }).click();
			const secret = base32Decode(await page.getByTestId('totp-secret').innerText());
			await page.getByLabel('Code').fill(totp(secret));
			await page.getByRole('button', { name: 'Turn on' }).click();
			await expect(page.getByText('Save your recovery codes')).toBeVisible();
			await expect(page.getByTestId('mfa-required')).toHaveCount(0);
			await page.goto('/');
			await expect(page.getByRole('heading', { level: 1 })).toHaveText(
				/^Good (morning|afternoon|evening), /
			);
		} finally {
			await api.put(`/api/v1/security/mfa-policy/roles/${readOnly.id}`, {
				data: { required: false }
			});
			await api.dispose();
		}
	});
});
