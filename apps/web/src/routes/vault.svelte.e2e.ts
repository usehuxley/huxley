import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the service-desk preset (Clients). The
// vault module is optional: the test turns it on.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** Turns the vault on and makes a client: its id. */
async function setUp(stamp: string) {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const on = await api.put('/api/v1/modules/vault', { data: { enabled: true } });
	expect(on.status(), await on.text()).toBe(200);
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `Vault ${stamp}`
		}
	});
	expect(client.status(), await client.text()).toBe(201);
	const { id } = (await client.json()) as { id: string };
	await api.dispose();
	return id;
}

test.describe('credential vault', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'clients are off in this run');

	test('a password is added, revealed with a reason, changed and deleted', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		await signIn(page);
		await page.goto(`/clients/${client}?tab=vault`);
		const panel = page.getByRole('tabpanel');
		await expect(panel.getByText('No passwords yet')).toBeVisible();

		await expect(async () => {
			await panel.getByRole('button', { name: 'Add', exact: true }).click();
			await expect(page.getByRole('dialog')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name', { exact: true }).fill('Firewall admin');
		await dialog.getByLabel('Username').fill('admin');
		await dialog.getByLabel('Address').fill('https://10.0.0.1');
		await dialog.getByLabel('Secret', { exact: true }).fill(`Hunter2-${stamp}`);
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Added.')).toBeVisible();

		// Never on the page until revealed; revealed with a reason, then hidden.
		const list = page.getByRole('list', { name: 'Passwords' });
		await expect(list).toContainText('Firewall admin');
		await expect(page.locator('body')).not.toContainText(`Hunter2-${stamp}`);
		await expect(list).toContainText('Never revealed');
		await list.getByRole('button', { name: 'Reveal the secret of Firewall admin' }).click();
		await page.getByLabel('Reason').fill('Ticket T-9');
		await page.getByRole('dialog').getByRole('button', { name: 'Reveal' }).click();
		await expect(page.getByTestId('vault-secret')).toContainText(`Hunter2-${stamp}`);
		await list.getByRole('button', { name: 'Hide the secret of Firewall admin' }).click();
		await expect(page.locator('body')).not.toContainText(`Hunter2-${stamp}`);
		await page.reload();
		await expect(page.getByRole('list', { name: 'Passwords' })).toContainText('Last revealed');

		// A new secret replaces the old.
		await page.getByRole('button', { name: 'Edit Firewall admin' }).click();
		await page.getByRole('dialog').getByLabel('Secret', { exact: true }).fill(`Rotated-${stamp}`);
		await page.getByRole('dialog').getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Saved.')).toBeVisible();
		await page.getByRole('button', { name: 'Reveal the secret of Firewall admin' }).click();
		await page.getByRole('dialog').getByRole('button', { name: 'Reveal' }).click();
		await expect(page.getByTestId('vault-secret')).toContainText(`Rotated-${stamp}`);

		// Deleted.
		await page.getByRole('button', { name: 'Edit Firewall admin' }).click();
		await page.getByRole('dialog').getByRole('button', { name: 'Delete' }).click();
		await expect(page.getByRole('tabpanel').getByText('No passwords yet')).toBeVisible();
	});

	test("a client's documentation lists its articles", async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		await signIn(page);
		await page.goto(`/clients/${client}?tab=documentation`);
		await expect(page.getByRole('tabpanel').getByText('No documentation yet.')).toBeVisible();
		await page.getByRole('link', { name: 'New document' }).click();
		await page.getByLabel('Title').fill(`Network ${stamp}`);
		// Written for this client.
		await expect(page.getByLabel('Organization', { exact: true })).toHaveValue(client);
		await page.getByRole('button', { name: 'Save draft' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Network ${stamp}`);
		await page.goto(`/clients/${client}?tab=documentation`);
		await expect(page.getByRole('list', { name: 'Documentation' })).toContainText(
			`Network ${stamp}`
		);
	});
});
