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
		// The dependency map shows both.
		const map = page.getByTestId('dependency-map');
		await expect(map).toContainText(`VM ${stamp}`);
		await expect(map).toContainText(`Host ${stamp}`);

		// Retired.
		await page.getByRole('link', { name: 'Edit' }).click();
		await page.getByLabel('Status').selectOption('retired');
		await page.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Host ${stamp}`);
		await expect(page.getByText('Retired', { exact: true })).toBeVisible();
		await expect(page.getByLabel('Details')).toContainText('256');
	});

	test('a ticket links an asset, which lists the ticket', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const ticket = await api.post('/api/v1/tickets', {
			data: { orgId: client, title: `VM slow ${stamp}` }
		});
		const t = (await ticket.json()) as { id: string; reference: string };
		await api.dispose();
		await signIn(page);
		await page.goto(`/tickets/${t.id}`);
		await expect(page.getByText('No assets linked.')).toBeVisible();
		await expect(async () => {
			await page.getByLabel('Find an asset to link').fill(`VM ${stamp}`);
			await expect(page.getByRole('list', { name: 'Assets found' })).toBeVisible({ timeout: 2000 });
		}).toPass({ timeout: 15_000 });
		await page
			.getByRole('list', { name: 'Assets found' })
			.getByRole('button', { name: `Link VM ${stamp}` })
			.click();
		const linked = page.getByRole('list', { name: 'Ticket assets' });
		await expect(linked).toContainText(`VM ${stamp}`);
		await linked.getByRole('link', { name: `VM ${stamp}` }).click();
		await expect(page.getByRole('list', { name: 'Asset tickets' })).toContainText(t.reference);
	});

	test('an asset type is added with its own fields, which new assets ask for', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		await signIn(page);
		await page.goto('/settings/asset-types');
		await expect(page.getByRole('list', { name: 'Asset types' })).toContainText('Workstation');
		await expect(async () => {
			await page.getByRole('button', { name: 'New type' }).click();
			await expect(page.getByRole('dialog')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name', { exact: true }).fill(`Door controller ${stamp}`);
		await dialog.getByRole('button', { name: 'Add a field' }).click();
		await dialog.getByRole('group', { name: 'Field 1' }).getByLabel('Label').fill('Zone');
		await dialog.getByRole('group', { name: 'Field 1' }).getByLabel('Required').check();
		await dialog.getByRole('button', { name: 'Add a field' }).click();
		const mode = dialog.getByRole('group', { name: 'Field 2' });
		await mode.getByLabel('Label').fill('Mode');
		await mode.getByLabel('Kind').selectOption('select');
		await mode.getByLabel('Options').fill('Locked\nUnlocked');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Type saved.')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Asset types' })).toContainText(
			`Door controller ${stamp}`
		);

		// A new asset of that type asks for its fields.
		await page.goto(`/assets/new?org=${client}`);
		await expect(async () => {
			await page
				.getByLabel('Type', { exact: true })
				.selectOption({ label: `Door controller ${stamp}` });
			await expect(page.getByLabel('Zone')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		await page.getByLabel('Name', { exact: true }).fill(`Front door ${stamp}`);
		await page.getByLabel('Zone').fill('Lobby');
		await page.getByLabel('Mode', { exact: true }).selectOption({ label: 'Unlocked' });
		await page.getByRole('button', { name: 'Add asset' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Front door ${stamp}`);
		await expect(page.getByLabel('Details')).toContainText('Lobby');
		await expect(page.getByLabel('Details')).toContainText('Unlocked');
	});

	test('a dataset is mapped to assets, previewed and synced', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const ds = await api.post('/api/v1/datasets', {
			data: {
				orgId: client,
				dataset: {
					key: `rmm-${stamp}`,
					name: `RMM ${stamp}`,
					primaryKey: ['device_id'],
					columns: [
						{ name: 'device_id', type: 'text' },
						{ name: 'hostname', type: 'text' }
					]
				}
			}
		});
		expect(ds.status(), await ds.text()).toBe(201);
		const { id: dsId } = (await ds.json()) as { id: string };
		const rows = await api.post(`/api/v1/datasets/${dsId}/rows`, {
			data: {
				orgId: client,
				rows: [
					{ device_id: `a-${stamp}`, hostname: `PC-A-${stamp}` },
					{ device_id: `b-${stamp}`, hostname: `PC-B-${stamp}` }
				]
			}
		});
		expect(rows.status(), await rows.text()).toBe(200);
		await api.dispose();

		await signIn(page);
		await page.goto('/settings/asset-sync');
		await expect(async () => {
			await page.getByRole('button', { name: 'New mapping' }).click();
			await expect(page.getByRole('dialog')).toBeVisible({ timeout: 1000 });
		}).toPass({ timeout: 15_000 });
		const dialog = page.getByRole('dialog');
		await dialog.locator('#map-name').fill(`RMM ${stamp}`);
		await dialog.getByLabel('Dataset').selectOption({ label: `RMM ${stamp}` });
		await dialog.getByLabel('New assets are').selectOption({ label: 'Workstation' });
		await dialog
			.getByRole('group', { name: 'Match key 1' })
			.getByLabel('Column')
			.selectOption('device_id');
		await dialog.locator('#mf-name').selectOption('hostname');
		await dialog.locator('#mf-hostname').selectOption('hostname');
		await dialog.locator('#mf-external_id').selectOption('device_id');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Mapping saved.')).toBeVisible();

		const mappings = page.getByRole('list', { name: 'Asset mappings' });
		await mappings.getByRole('button', { name: `Preview RMM ${stamp}` }).click();
		await expect(page.getByText('Preview: 2 rows: 2 new').first()).toBeVisible();
		await mappings.getByRole('button', { name: `Sync RMM ${stamp} now` }).click();
		await expect(page.getByText('Synced: 2 rows: 2 new').first()).toBeVisible();

		// The asset shows where its values came from.
		await page.goto(`/assets?q=PC-A-${stamp}`);
		await page.getByRole('link', { name: `PC-A-${stamp}` }).click();
		await expect(page.getByTestId('asset-sync')).toContainText(`rmm: a-${stamp}`);
		await expect(page.getByLabel('Details')).toContainText('rmm');
	});

	test('a purchase date and a warranty are kept, and ending warranties found', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const client = await setUp(stamp);
		const soon = new Date(Date.now() + 20 * 86_400_000).toISOString().slice(0, 10);
		await signIn(page);
		await page.goto(`/assets/new?org=${client}`);
		await page.getByLabel('Name', { exact: true }).fill(`Laptop ${stamp}`);
		await page.getByLabel('Purchased on').fill('2025-03-01');
		await page.getByRole('button', { name: 'Add asset' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Laptop ${stamp}`);
		await expect(page.getByLabel('Details')).toContainText('2025-03-01');

		const warranties = page.getByRole('list', { name: 'Warranties' });
		await page.getByLabel('Provider').fill('Dell');
		await page.getByLabel('Reference').fill(`SVC-${stamp}`);
		await page.getByLabel('Ends', { exact: true }).fill(soon);
		await page.getByRole('button', { name: 'Add warranty' }).click();
		await expect(warranties).toContainText('Manufacturer · Dell');
		await expect(warranties).toContainText('(in 20 days)');
		await expect(page.getByLabel('Details')).toContainText(soon);

		await page.goto(`/assets?q=Laptop%20${stamp}&warranty=30`);
		await expect(page.getByRole('table', { name: 'Assets' })).toContainText(`Laptop ${stamp}`);
		await page.goto(`/assets?q=Laptop%20${stamp}&warranty=30`);
		await page.getByRole('link', { name: `Laptop ${stamp}` }).click();
		await page.getByRole('button', { name: `Remove the warranty ending ${soon}` }).click();
		await expect(page.getByText('None recorded.')).toBeVisible();
	});
});
