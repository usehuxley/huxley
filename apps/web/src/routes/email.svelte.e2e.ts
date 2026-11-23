import { expect, request, test } from '@playwright/test';

// Full-stack with real SMTP: needs the API configured to send to Mailpit
// (HUXLEY_EMAIL__SMTP_URL=smtp://127.0.0.1:1025) and MAILPIT_URL pointing at
// Mailpit's web API (http://127.0.0.1:8025). CI runs both.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;

/** The link in the newest email to `to`, waiting for it to arrive. */
async function linkIn(to: string, path: string): Promise<string> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 50; i++) {
			const search = await api.get(`/api/v1/search?query=${encodeURIComponent(`to:"${to}"`)}`);
			const { messages } = (await search.json()) as { messages: { ID: string }[] };
			if (messages.length > 0) {
				const message = (await (await api.get(`/api/v1/message/${messages[0].ID}`)).json()) as {
					Text: string;
				};
				const match = message.Text.match(new RegExp(`https?://\\S+${path}\\?token=\\S+`));
				if (match) return match[0];
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no email to ${to}`);
	} finally {
		await api.dispose();
	}
}

/** Waits for an email to `to` with `subject` (e.g. one the worker sends). */
async function expectEmail(to: string, subject: string) {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 50; i++) {
			const query = `to:"${to}" subject:"${subject}"`;
			const res = await api.get(`/api/v1/search?query=${encodeURIComponent(query)}`);
			if (((await res.json()) as { messages: unknown[] }).messages.length > 0) return;
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no "${subject}" email to ${to}`);
	} finally {
		await api.dispose();
	}
}

/** Creates a user through the API as the owner. */
async function createUser(body: Record<string, unknown>) {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const res = await api.post('/api/v1/users', { data: body });
	expect(res.status()).toBe(201);
	await api.dispose();
}

test.describe('email', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL, and MAILPIT_URL with an API that sends to Mailpit'
	);

	test('a forgotten password is reset from the emailed link', async ({ page }) => {
		const email = `forgetful-${Date.now().toString(36)}@example.com`;
		await createUser({ email, displayName: 'Fran Forgetful', password: 'my original passphrase' });

		await page.goto('/login');
		await page.getByRole('link', { name: 'Forgot password?' }).click();
		await expect(page.getByRole('heading', { name: 'Reset your password' })).toBeVisible();
		await page.getByLabel('Email').fill(email);
		await page.getByRole('button', { name: 'Send reset link' }).click();
		await expect(page.getByText('a reset link is on its way')).toBeVisible();

		const link = new URL(await linkIn(email, '/reset-password'));
		await page.goto(link.pathname + link.search);
		await page.getByLabel('New password').fill('my replacement passphrase');
		await page.getByLabel('Confirm password').fill('my replacement passphrase');
		await page.getByRole('button', { name: 'Set password' }).click();
		await expect(page.getByText('Your password is set. Sign in with it.')).toBeVisible();

		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill('my replacement passphrase');
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
		await page.goto('/profile');
		await expect(page.getByText('Verified', { exact: true })).toBeVisible();

		// The worker emails a security notice about the change.
		await expectEmail(email, 'Your password was reset');
	});

	test('an invited user chooses their own password', async ({ page }) => {
		const email = `invited-${Date.now().toString(36)}@example.com`;
		await createUser({ email, displayName: 'Indy Invited', invite: true });
		const link = new URL(await linkIn(email, '/reset-password'));
		await page.goto(link.pathname + link.search);
		await page.getByLabel('New password').fill('a passphrase I chose');
		await page.getByLabel('Confirm password').fill('a passphrase I chose');
		await page.getByRole('button', { name: 'Set password' }).click();
		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill('a passphrase I chose');
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	});
});
