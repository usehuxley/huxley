import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API and the service-desk preset (tickets). Set
// SCREENSHOT_DIR to keep screenshots of the dashboards.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';
const shots = process.env.SCREENSHOT_DIR;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

test.describe('dashboards', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a prebuilt dashboard shows numbers, and a copy gets a new widget', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
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
				name: `Dash ${stamp}`
			}
		});
		const { id: clientId } = (await client.json()) as { id: string };
		for (const [title, priority] of [
			['Printer jam', 'p3'],
			['VPN down', 'p1'],
			['New laptop', 'p4']
		]) {
			const t = await api.post('/api/v1/tickets', { data: { orgId: clientId, title, priority } });
			expect(t.status(), await t.text()).toBe(201);
		}
		await api.dispose();

		await signIn(page);
		await page.getByRole('link', { name: 'Dashboards' }).first().click();
		await page.getByRole('link', { name: /Service desk overview/ }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Service desk overview');
		// Narrowed to the new client: three open tickets, by priority.
		await page.getByLabel('Show numbers for').selectOption({ label: `Dash ${stamp}` });
		const open = page.getByRole('region', { name: 'Open tickets' });
		await expect(open.getByText('3', { exact: true })).toBeVisible();
		const byPriority = page.getByRole('region', { name: 'Open by priority' });
		await expect(byPriority.getByRole('img')).toBeVisible();
		await byPriority.getByRole('button', { name: 'Table' }).click();
		await expect(byPriority.getByRole('table').getByText('p1')).toBeVisible();
		if (shots) await page.screenshot({ path: `${shots}/service-desk.png`, fullPage: true });

		// A copy can be changed: add a donut of open tickets by priority.
		await page.getByRole('button', { name: 'Copy to change it' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Edit dashboard');
		await page.getByLabel('Name', { exact: true }).fill(`My desk ${stamp}`);
		await page.getByRole('button', { name: 'Add a widget' }).click();
		const dialog = page.getByRole('dialog');
		await dialog.getByLabel('Title').fill('Priority mix');
		await dialog.getByLabel('Show as').selectOption('pie');
		await dialog.getByLabel('Grouped by').selectOption('priority');
		await dialog.getByRole('button', { name: 'Done' }).click();
		await expect(
			page.getByRole('list', { name: 'Widgets' }).getByText('Priority mix')
		).toBeVisible();
		await page.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`My desk ${stamp}`);
		await page.getByLabel('Show numbers for').selectOption({ label: `Dash ${stamp}` });
		await expect(
			page.getByRole('region', { name: 'Priority mix' }).getByRole('list', { name: 'Legend' })
		).toContainText('p1');
		if (shots) await page.screenshot({ path: `${shots}/copy.png`, fullPage: true });
	});
});
