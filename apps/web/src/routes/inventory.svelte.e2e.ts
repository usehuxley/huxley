import { expect, request, test, type APIResponse, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the service-desk preset (tickets). The
// contracts and inventory modules are turned on here.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

test.describe('inventory', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'tickets are off in this run');

	test('a product is stocked, received, used on a ticket, and runs low', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		const json = async <T>(res: Promise<APIResponse>) => {
			const r = await res;
			expect(r.ok(), await r.text()).toBeTruthy();
			return (await r.json()) as T;
		};
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'inventory']) {
			await json(api.put(`/api/v1/modules/${module}`, { data: { enabled: true } }));
		}
		const orgs = await json<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		await json(
			api.post('/api/v1/catalog/products', {
				data: {
					orgId: root,
					product: {
						sku: `SSD-${stamp}`,
						name: `SSD ${stamp}`,
						kind: 'product',
						cost: '40',
						price: '80'
					}
				}
			})
		);
		const client = await json<{ id: string }>(
			api.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Umbrella ${stamp}` }
			})
		);
		const ticket = await json<{ id: string }>(
			api.post('/api/v1/tickets', { data: { orgId: client.id, title: `Laptop upgrade ${stamp}` } })
		);
		await api.dispose();

		await signIn(page);
		// A warehouse to keep it in.
		await page.goto('/settings/inventory');
		await page.getByRole('button', { name: 'New location' }).click();
		const loc = page.getByRole('dialog', { name: 'New location' });
		await loc.getByLabel('Name').fill(`Main ${stamp}`);
		await loc.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByRole('list', { name: 'Locations' })).toContainText(`Main ${stamp}`);

		await openFromMenu(page, 'Inventory');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Inventory');
		await page.getByRole('button', { name: 'Stock a product' }).click();
		const stock = page.getByRole('dialog', { name: 'Stock a product' });
		await stock.getByLabel('Product').selectOption({ label: `SSD ${stamp} · SSD-${stamp}` });
		await stock.getByRole('button', { name: 'Stock it' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`SSD ${stamp}`);

		await page.getByRole('button', { name: 'Receive' }).click();
		const receive = page.getByRole('dialog', { name: 'Receive' });
		await receive.getByLabel('Location').selectOption({ label: `Main ${stamp}` });
		await receive.getByLabel('Quantity').fill('10');
		await receive.getByLabel('Unit cost').fill('40');
		await receive.getByRole('button', { name: 'Record' }).click();
		const where = page.getByRole('table', { name: 'Stock by location' });
		await expect(where).toContainText(`Main ${stamp}`);
		await expect(where).toContainText('10');

		await where.getByRole('button', { name: 'Reorder point' }).click();
		const reorder = page.getByRole('dialog', { name: 'Reorder point' });
		await reorder.getByLabel('Reorder when available falls to').fill('5');
		await reorder.getByRole('button', { name: 'Save' }).click();
		await expect(where).toContainText('5');

		// Used on a ticket.
		await page.goto(`/tickets/${ticket.id}`);
		await page.getByRole('button', { name: 'Add part' }).click();
		const add = page.getByRole('dialog', { name: 'Add a part' });
		await add
			.getByLabel('Item')
			.selectOption({ label: `SSD ${stamp} · SSD-${stamp} (10 available)` });
		await add.getByLabel('Quantity').fill('6');
		await add.getByLabel('Used already (not just set aside)').check();
		await add.getByRole('button', { name: 'Add' }).click();
		const parts = page.getByRole('list', { name: 'Parts' });
		await expect(parts).toContainText(`6 × SSD ${stamp}`);
		await expect(parts).toContainText('Used');

		// Back on the item: four left, and it's running low.
		await parts.getByRole('link', { name: `6 × SSD ${stamp}` }).click();
		await expect(page.getByLabel('Totals')).toContainText('4');
		await expect(page.getByText('Running low')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Movements' })).toContainText('Used');
	});
});
