import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the service-desk preset (crm, portal).
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

/** A client with a contact: the contact's id. */
async function setUp(stamp: string) {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const client = await api.post('/api/v1/orgs', {
		data: {
			parentId: orgs.find((o) => o.parentId === null)!.id,
			kind: 'client',
			name: `Portal ${stamp}`
		}
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Pat', lastName: stamp, email: `pat-${stamp}@example.com` }
	});
	expect(contact.status(), await contact.text()).toBe(201);
	const { id } = (await contact.json()) as { id: string };
	await api.dispose();
	return id;
}

test.describe('portal access', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm and portal modules are off in this run');

	test('a contact is invited to the portal, given a role, and disabled', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const contact = await setUp(stamp);
		await signIn(page);
		await page.goto(`/contacts/${contact}`);
		await expect(page.getByText('No portal access yet.')).toBeVisible();
		await page.getByLabel('Portal role').selectOption('client_admin');
		await page.getByRole('button', { name: 'Invite to the portal' }).click();
		await expect(page.getByText('They have portal access.', { exact: false })).toBeVisible();
		await expect(page.getByText('Invited', { exact: true })).toBeVisible();
		await expect(
			page.getByText(`Signs in as pat-${stamp}@example.com`, { exact: false })
		).toBeVisible();

		await page.getByRole('button', { name: 'Disable portal access' }).click();
		await expect(page.getByText('Portal access disabled.')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Re-enable' })).toBeVisible();
	});
});
