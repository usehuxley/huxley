import { expect, request, test, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the service-desk preset (crm). The
// contracts module is optional: the test turns it on.
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

test.describe('product catalog', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm module is off in this run');

	test('products and a bundle are added, priced and retired', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const on = await api.put('/api/v1/modules/contracts', { data: { enabled: true } });
		expect(on.status(), await on.text()).toBe(200);
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			name: string;
			parentId: string | null;
		}[];
		const msp = orgs.find((o) => o.parentId === null)!;
		const laptop = await api.post('/api/v1/catalog/products', {
			data: {
				orgId: msp.id,
				product: {
					sku: `LAT-${stamp}`,
					name: `Laptop ${stamp}`,
					kind: 'product',
					vendor: 'Dell',
					cost: '800',
					price: '1000.00'
				}
			}
		});
		expect(laptop.status(), await laptop.text()).toBe(201);
		await api.dispose();

		await signIn(page);
		await openFromMenu(page, 'Catalog');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Catalog');

		// A service, by hand.
		const dialog = page.getByRole('dialog');
		await page.getByRole('button', { name: 'New product' }).click();
		await dialog.locator('#p-name').fill(`Setup ${stamp}`);
		await dialog.locator('#p-sku').fill(`SETUP-${stamp}`);
		await dialog.locator('#p-kind').selectOption('service');
		if (await dialog.locator('#p-org').count()) {
			await dialog.locator('#p-org').selectOption({ label: msp.name });
		}
		await dialog.locator('#p-unit').fill('hour');
		await dialog.locator('#p-price').fill('125.50');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Product added.')).toBeVisible();

		// A bundle of both, priced by its components.
		await page.getByRole('button', { name: 'New product' }).click();
		await dialog.locator('#p-name').fill(`Kit ${stamp}`);
		await dialog.locator('#p-sku').fill(`KIT-${stamp}`);
		await dialog.locator('#p-kind').selectOption('bundle');
		if (await dialog.locator('#p-org').count()) {
			await dialog.locator('#p-org').selectOption({ label: msp.name });
		}
		await dialog.getByRole('button', { name: 'Add a product' }).click();
		await dialog.locator('#part-0').selectOption({ label: `Laptop ${stamp} (LAT-${stamp})` });
		await dialog.getByRole('button', { name: 'Add a product' }).click();
		await dialog.locator('#part-1').selectOption({ label: `Setup ${stamp} (SETUP-${stamp})` });
		await dialog.locator('#qty-1').fill('2');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Product added.')).toBeVisible();

		await page.getByLabel('Search the catalog').fill(stamp);
		const table = page.getByRole('table', { name: 'Products' });
		const kit = table.getByRole('row').filter({ hasText: `Kit ${stamp}` });
		await expect(kit).toContainText('1 × Laptop');
		await expect(kit).toContainText('2 × Setup');
		await expect(kit).toContainText('1,251');

		// Retired: hidden unless asked for.
		await table.getByRole('button', { name: `Laptop ${stamp}` }).click();
		await dialog.getByLabel('Offered (untick to retire it)').uncheck();
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Product saved.')).toBeVisible();
		await expect(table.getByRole('button', { name: `Laptop ${stamp}` })).toHaveCount(0);
		await page.getByLabel('Show retired').check();
		const retired = table.getByRole('row').filter({
			has: page.getByRole('button', { name: `Laptop ${stamp}`, exact: true })
		});
		await expect(retired).toContainText('Retired');
	});
});
