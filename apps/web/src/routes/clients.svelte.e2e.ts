import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
// Runs only where the crm module is on (the e2e job's service-desk preset);
// with it off the suite runs as an install without it.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
// The preset the e2e instance was bootstrapped with (the CI job's matrix).
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** A new client organization, named `name`. */
async function newClient(name: string): Promise<string> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const created = await api.post('/api/v1/orgs', {
		data: { parentId: orgs.find((o) => o.parentId === null)!.id, kind: 'client', name }
	});
	expect(created.status()).toBe(201);
	const id = ((await created.json()) as { id: string }).id;
	await api.dispose();
	return id;
}

test.describe('clients and contacts', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm module is off in this run');

	test('a client gets details, a site and a contact', async ({ page }) => {
		await signIn(page);
		// With the module on, the nav shows Clients (not "Clients Soon").
		const live = page.getByRole('link', { name: 'Clients', exact: true });

		const name = `Acme ${Date.now().toString(36)}`;
		await newClient(name);
		await live.click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Clients');
		await page.getByLabel('Search clients').fill(name);
		await page.getByLabel('Search clients').press('Enter');
		await page.getByRole('link', { name }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(name);

		// Company details.
		await page.getByLabel('Status').selectOption('onboarding');
		await page.getByLabel('Website').fill('https://acme.example');
		await page.getByRole('button', { name: 'Save details' }).click();
		await expect(page.getByText('Details saved.')).toBeVisible();
		await expect(page.getByText('Onboarding', { exact: true }).first()).toBeVisible();

		// A site, with its hours.
		await page.getByRole('tab', { name: /Sites/ }).click();
		await page.getByRole('button', { name: 'Add site' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name').fill('Head office');
		await dialog.getByLabel('City').fill('London');
		await dialog.getByLabel('Country').fill('GB');
		await dialog.getByLabel('Timezone').fill('Europe/London');
		await dialog.getByLabel('Friday closes').fill('16:00');
		await dialog.getByRole('button', { name: 'Save site' }).click();
		const sites = page.getByRole('list', { name: 'Sites' });
		await expect(sites).toContainText('Head office');
		await expect(sites).toContainText('Primary');
		await expect(sites).toContainText('Mon–Thu 09:00–17:00; Fri 09:00–16:00');

		// A contact, then their page.
		await page.getByRole('tab', { name: /Contacts/ }).click();
		await page.getByRole('button', { name: 'Add contact' }).click();
		await dialog.getByLabel('First name').fill('Ada');
		await dialog.getByLabel('Last name').fill('Lovelace');
		await dialog.getByLabel('Email').fill('ada@acme.example');
		await dialog.getByLabel('Job title').fill('CTO');
		await dialog.getByRole('button', { name: 'Add contact' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Ada Lovelace');
		await expect(page.getByText('CTO').first()).toBeVisible();

		await page.getByLabel('Mobile').fill('+44 7700 900000');
		await page.getByLabel('Site').selectOption({ label: 'Head office' });
		await page.getByRole('button', { name: 'Save contact' }).click();
		await expect(page.getByText('Contact saved.')).toBeVisible();

		// Back on the client, they're listed and found by search.
		await page.getByRole('link', { name: `← ${name}` }).click();
		await page.getByRole('tab', { name: /Contacts/ }).click();
		await page.getByLabel('Search contacts').fill('lovelace');
		await page.getByLabel('Search contacts').press('Enter');
		await expect(page.getByRole('table', { name: 'Contacts' })).toContainText('ada@acme.example');
	});
});
