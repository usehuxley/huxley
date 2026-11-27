import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner, HUXLEY_API_URL, and
// HUXLEY_AUTH__PUBLIC_URL=http://localhost:4173 on the API (see the CI job).
// Chromium's virtual authenticator stands in for a platform passkey.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const PASSWORD = 'passkey e2e passphrase';

async function createUser(): Promise<string> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	expect(
		(
			await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } })
		).ok()
	).toBe(true);
	const methods = (await (await api.get('/api/v1/auth/methods')).json()) as { passkeys: boolean };
	test.skip(!methods.passkeys, 'the API has no auth.public_url, so passkeys are off');
	const email = `passkey-${Date.now().toString(36)}@example.com`;
	const created = await api.post('/api/v1/users', {
		data: { email, displayName: 'Pat Passkey', password: PASSWORD }
	});
	expect(created.status()).toBe(201);
	await api.dispose();
	return email;
}

/** Gives the page a device with a fingerprint reader that always matches. */
async function addVirtualAuthenticator(page: Page) {
	const cdp = await page.context().newCDPSession(page);
	await cdp.send('WebAuthn.enable');
	await cdp.send('WebAuthn.addVirtualAuthenticator', {
		options: {
			protocol: 'ctap2',
			transport: 'internal',
			hasResidentKey: true,
			hasUserVerification: true,
			isUserVerified: true,
			automaticPresenceSimulation: true
		}
	});
}

async function signOut(page: Page) {
	await page.getByRole('button', { name: 'Sign out' }).click();
	await expect(page).toHaveURL(/\/login$/);
}

test.describe('passkeys', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL with a running API'
	);

	test('add a passkey, use it as a second factor, then sign in with it alone', async ({ page }) => {
		const email = await createUser();
		await addVirtualAuthenticator(page);

		await page.goto('/login');
		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill(PASSWORD);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		// Add a passkey from the profile. It's the first second factor, so
		// recovery codes come with it.
		await page.goto('/profile');
		await page.locator('#passkeyName').fill('Test laptop');
		await page.locator('#passkeyPassword').fill(PASSWORD);
		await page.getByRole('button', { name: 'Add a passkey' }).click();
		await expect(page.getByText('Save your recovery codes')).toBeVisible();
		await expect(page.getByText('Test laptop')).toBeVisible();
		await expect(page.getByText('10 recovery codes left')).toBeVisible();

		// Password sign-in now asks for the passkey.
		await signOut(page);
		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill(PASSWORD);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await page.getByRole('button', { name: 'Use a passkey' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		// And the passkey alone signs in, returning to the requested page.
		await signOut(page);
		await page.goto('/profile');
		await expect(page).toHaveURL(/\/login\?next=%2Fprofile$/);
		await page.getByRole('button', { name: 'Sign in with a passkey' }).click();
		await expect(page).toHaveURL(/\/profile$/);
		await expect(page.getByText(/Last used (?!—)/)).toBeVisible();
	});
});
