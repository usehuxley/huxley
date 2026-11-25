import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the cmdb module (the service-desk preset).
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

/** A client with one asset, a VM: their ids. */
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
			name: `Assets ${stamp}`
		}
	});
	const { id } = (await client.json()) as { id: string };
	const types = (await (await api.get('/api/v1/asset-types')).json()) as {
		id: string;
		key: string;
	}[];
	const vm = await api.post('/api/v1/assets', {
		data: { orgId: id, typeId: types.find((t) => t.key === 'server')!.id, name: `VM ${stamp}` }
	});
	expect(vm.status(), await vm.text()).toBe(201);
	await api.dispose();
	return id;
}

test.describe('assets', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the cmdb module is off in this run');

	test('an asset is added with its type fields, related, and its impact shown', async ({
		page
	}) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		await signIn(page);
		await page.goto(`/clients/${client}?tab=assets`);
		await expect(page.getByRole('list', { name: 'Client assets' })).toContainText(`VM ${stamp}`);
		await page.getByRole('link', { name: 'New asset' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			`New asset for Assets ${stamp}`
		);

		await page.getByLabel('Name', { exact: true }).fill(`Host ${stamp}`);
		// The type's own fields follow the type.
		await expect(async () => {
			await page.getByLabel('Type', { exact: true }).selectOption({ label: 'Server' });
			await expect(page.getByLabel('Role')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		await page.getByLabel('Memory (GB)').fill('256');
		await page.getByLabel('Virtual machine').uncheck();
		await page.getByLabel('Serial number').fill(`SN-${stamp}`);
		await page.getByRole('button', { name: 'Add asset' }).click();

		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Host ${stamp}`);
		const details = page.getByRole('definition');
		await expect(page.getByLabel('Details')).toContainText('256');
		await expect(details.filter({ hasText: `SN-${stamp}` })).toHaveCount(1);

		// Host hosts the VM: the VM is what breaks if the host goes down.
		await page.getByLabel('Relationship').selectOption('hosts');
		await page.getByLabel('Asset', { exact: true }).selectOption({ label: `VM ${stamp} (Server)` });
		await page.getByRole('button', { name: 'Add', exact: true }).click();
		await expect(page.getByRole('list', { name: 'Relationships' })).toContainText(
			`hosts VM ${stamp}`
		);
		await expect(page.getByRole('list', { name: 'Impact' })).toContainText(`VM ${stamp}`);

		// Retired.
		await page.getByRole('link', { name: 'Edit' }).click();
		await page.getByLabel('Status').selectOption('retired');
		await page.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Host ${stamp}`);
		await expect(page.getByText('Retired', { exact: true })).toBeVisible();
		await expect(page.getByLabel('Details')).toContainText('256');
	});
});
