import { expect, request, test, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the service-desk preset (crm). The contracts
// and sales modules are optional: the test turns them on.
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

test.describe('sales pipeline', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm module is off in this run');

	test('a lead is converted, moved through the pipeline and won', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const company = `Initech ${stamp}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'sales']) {
			const on = await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
			expect(on.status(), await on.text()).toBe(200);
		}
		await api.dispose();

		await signIn(page);
		await openFromMenu(page, 'Leads');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Leads');
		await page.getByRole('button', { name: 'New lead' }).click();
		const add = page.getByRole('dialog', { name: 'New lead' });
		await add.getByLabel('Company').fill(company);
		await add.getByLabel('First name').fill('Peter');
		await add.getByLabel('Last name').fill('Gibbons');
		await add.getByLabel('Email').fill(`peter-${stamp}@initech.example`);
		await add.getByLabel('Source').fill('Referral');
		await add.getByRole('button', { name: 'Add lead' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(company);

		// Converting makes the client, the contact and the opportunity.
		await page.getByRole('button', { name: 'Convert' }).click();
		const convert = page.getByRole('dialog', { name: `Convert ${company}` });
		await convert.getByLabel('Name', { exact: true }).fill(`Managed services ${stamp}`);
		await convert.getByLabel('Monthly recurring').fill('2400');
		await convert.getByLabel('One-time value').fill('5000');
		await convert.getByRole('button', { name: 'Convert' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Managed services ${stamp}`);
		await expect(page.getByLabel('Contact')).toHaveText(/Peter Gibbons/);

		// Moving along the stages keeps history.
		const stages = page.getByRole('group', { name: 'Stage' });
		await stages.getByRole('button').nth(1).click();
		await expect(
			page.getByRole('list', { name: 'Stage history' }).getByRole('listitem')
		).toHaveCount(2);

		// Won, with a reason.
		await page.getByRole('button', { name: 'Won', exact: true }).click();
		const won = page.getByRole('dialog', { name: 'Mark it won' });
		await won.getByLabel('Reason').selectOption({ label: 'Relationship' });
		await won.getByLabel('Note').fill('They liked our response times');
		await won.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('They liked our response times')).toBeVisible();
		await expect(page.getByText('100% likely')).toBeVisible();

		// It's on the list of won deals, with its client.
		await openFromMenu(page, 'Pipeline');
		await page.getByRole('button', { name: 'List' }).click();
		await page.getByLabel('Status').selectOption('won');
		const row = page
			.getByRole('table', { name: 'Opportunities' })
			.getByRole('row')
			.filter({ hasText: `Managed services ${stamp}` });
		await expect(row).toContainText(company);
		await expect(row).toContainText('$5,000');
	});
});

test.describe('quotes', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the crm module is off in this run');

	test('a quote is built from the catalog, sent and accepted, which wins the deal', async ({
		page
	}) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'sales']) {
			await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
		}
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const product = await api.post('/api/v1/catalog/products', {
			data: {
				orgId: root,
				product: {
					sku: `FG-${stamp}`,
					name: `Firewall ${stamp}`,
					kind: 'product',
					cost: '800',
					price: '1000'
				}
			}
		});
		expect(product.status(), await product.text()).toBe(201);
		const client = (await (
			await api.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Contoso ${stamp}` }
			})
		).json()) as { id: string };
		const opp = (await (
			await api.post('/api/v1/opportunities', {
				data: { orgId: client.id, opportunity: { name: `Firewall refresh ${stamp}` } }
			})
		).json()) as { id: string };
		await api.dispose();

		await signIn(page);
		await page.goto(`/sales/opportunities/${opp.id}`);
		await page.getByRole('link', { name: 'New quote' }).click();
		await page.getByRole('button', { name: 'Start the quote' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Firewall refresh ${stamp}`);

		// Lines: one from the catalog, one written in, monthly.
		await page
			.getByLabel('Add from the catalog')
			.selectOption({ label: `Firewall ${stamp} · FG-${stamp} · $1,000` });
		await page.getByLabel('Quantity').first().fill('2');
		await page.getByRole('button', { name: 'Line', exact: true }).click();
		await page.getByLabel('Item name').nth(1).fill('Managed firewall');
		await page.getByLabel('Billing').nth(1).selectOption('monthly');
		await page.getByLabel('Unit price').nth(1).fill('25');
		await page.getByLabel('Unit cost').nth(1).fill('15');
		const totals = page.getByLabel('Totals');
		await expect(totals).toContainText('$2,000');
		await expect(totals).toContainText('$25');
		await page.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Unsaved changes')).toBeHidden();

		await page.getByRole('button', { name: 'Mark sent' }).click();
		await expect(page.getByRole('list', { name: 'Revisions' })).toContainText('Revision 1');
		await page.getByRole('button', { name: 'Accepted', exact: true }).click();
		const dialog = page.getByRole('dialog', { name: 'Record the acceptance' });
		await dialog.getByLabel('Accepted by').fill('Rosa Park');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Accepted by Rosa Park')).toBeVisible();

		// The opportunity is won.
		await page
			.getByRole('link', { name: `Firewall refresh ${stamp}` })
			.first()
			.click();
		await expect(page.getByText('100% likely')).toBeVisible();
		await expect(page.getByRole('list', { name: 'Quotes' })).toContainText('Accepted');
	});

	test('a quote that needs review is sent back for changes, with comments', async ({ page }) => {
		// It waits on the review workflow twice; give a busy runner room.
		test.setTimeout(90_000);
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const title = `Review me ${stamp}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		const json = async <T>(res: Promise<import('@playwright/test').APIResponse>, status = 0) => {
			const r = await res;
			if (status) expect(r.status(), await r.text()).toBe(status);
			else expect(r.ok(), await r.text()).toBeTruthy();
			return (await r.json()) as T;
		};
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'sales']) {
			await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
		}
		const me = (await json<{ id: string }>(api.get('/api/v1/auth/me'))).id;
		const orgs = await json<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = await json<{ id: string }>(
			api.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Fabrikam ${stamp}` }
			})
		);
		// Rules are instance-wide: this one matches only this test's quote.
		const rule = await json<{ id: string }>(
			api.post('/api/v1/sales/review-rules', {
				data: {
					name: `Review ${stamp}`,
					kind: 'expression',
					expression: `quote.title == '${title}'`,
					reviewers: `user:${me}`
				}
			}),
			201
		);
		// The Quote review pack, at the client.
		const pack = await json<{ uploadId: string; version: string }>(
			api.post(`/api/v1/packs/builtin/huxley/quote-review?orgId=${client.id}`)
		);
		const installed = await json<{ items: { workflows: Record<string, string> } }>(
			api.post(`/api/v1/packs/uploads/${pack.uploadId}/install`, {
				data: { orgId: client.id, version: pack.version }
			})
		);
		for (const id of Object.values(installed.items.workflows)) {
			await json(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }), 200);
		}
		const quote = await json<{ id: string }>(
			api.post('/api/v1/quotes', {
				data: {
					orgId: client.id,
					quote: {
						title,
						lines: [{ name: 'Rack server', unitCost: '900', unitPrice: '1000' }]
					}
				}
			}),
			201
		);

		try {
			await signIn(page);
			await page.goto(`/sales/quotes/${quote.id}`);
			await expect(page.getByText('Needs review', { exact: true })).toBeVisible();
			await expect(page.getByRole('list', { name: 'Why it needs review' })).toContainText(
				`Review ${stamp}`
			);
			await expect(page.getByRole('button', { name: 'Mark sent' })).toBeHidden();

			// A comment on the line, mentioning nobody but noting it.
			await page.getByLabel('On', { exact: true }).selectOption({ label: 'Rack server' });
			await page.getByLabel('Add a comment').fill('Is the margin right?');
			await page.getByRole('button', { name: 'Comment', exact: true }).click();
			const open = page.getByRole('list', { name: 'Open comments' });
			await expect(open).toContainText('On Rack server');
			await expect(open).toContainText('Is the margin right?');
			await expect(page.getByText('1 open comment')).toBeVisible();

			await page.getByRole('button', { name: 'Submit for review' }).click();
			await expect(page.getByText('In review', { exact: true })).toBeVisible();
			// The workflow asks the reviewer (the owner, here).
			await expect(async () => {
				await page.reload();
				await expect(page.getByText('Your review')).toBeVisible({ timeout: 1000 });
			}).toPass({ timeout: 30_000 });
			await page.getByLabel('Review comment').fill('Price it at 1100');
			await page.getByRole('button', { name: 'Request changes' }).click();
			await expect(async () => {
				await page.reload();
				await expect(page.getByText('Changes requested', { exact: true }).first()).toBeVisible({
					timeout: 1000
				});
			}).toPass({ timeout: 30_000 });
			await expect(page.getByRole('list', { name: 'Review history' })).toContainText(
				'Price it at 1100'
			);

			// Resolving the thread tidies it away.
			await open.getByRole('button', { name: 'Resolve' }).click();
			await expect(page.getByText('No open comments.')).toBeVisible();
		} finally {
			await api.delete(`/api/v1/sales/review-rules/${rule.id}`);
			await api.dispose();
		}
	});

	test('an accepted quote becomes an order, delivered from stock and handed to an agreement', async ({
		page
	}) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		const json = async <T>(res: Promise<import('@playwright/test').APIResponse>) => {
			const r = await res;
			expect(r.ok(), await r.text()).toBeTruthy();
			return (await r.json()) as T;
		};
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'sales', 'inventory']) {
			await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
		}
		const orgs = await json<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
		const root = orgs.find((o) => o.parentId === null)!.id;
		const product = await json<{ id: string }>(
			api.post('/api/v1/catalog/products', {
				data: {
					orgId: root,
					product: {
						sku: `NUC-${stamp}`,
						name: `Mini PC ${stamp}`,
						kind: 'product',
						cost: '400',
						price: '600'
					}
				}
			})
		);
		const item = await json<{ id: string }>(
			api.post('/api/v1/inventory/items', {
				data: { orgId: root, item: { productId: product.id } }
			})
		);
		const shelf = await json<{ id: string }>(
			api.post('/api/v1/inventory/locations', {
				data: { orgId: root, location: { name: `Shelf ${stamp}`, kind: 'warehouse' } }
			})
		);
		await json(
			api.post('/api/v1/inventory/movements', {
				data: { kind: 'receive', itemId: item.id, locationId: shelf.id, quantity: 2 }
			})
		);
		const client = await json<{ id: string }>(
			api.post('/api/v1/orgs', { data: { parentId: root, kind: 'client', name: `Hooli ${stamp}` } })
		);
		const quote = await json<{ id: string; version: number }>(
			api.post('/api/v1/quotes', {
				data: {
					orgId: client.id,
					quote: {
						title: `Kiosks ${stamp}`,
						lines: [
							{ productId: product.id, quantity: 2 },
							{ name: 'Managed kiosk', billing: 'monthly', quantity: 2, unitPrice: '30' }
						]
					}
				}
			})
		);
		const sent = await json<{ version: number }>(
			api.post(`/api/v1/quotes/${quote.id}/send`, { data: { version: quote.version } })
		);
		await json(
			api.post(`/api/v1/quotes/${quote.id}/decision`, {
				data: { accept: true, name: 'Gavin Belson', version: sent.version }
			})
		);
		await api.dispose();

		await signIn(page);
		await openFromMenu(page, 'Orders');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Orders');
		await page.getByRole('link', { name: `Kiosks ${stamp}` }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Kiosks ${stamp}`);
		const lines = page.getByRole('region', { name: 'Lines' });
		await expect(lines).toContainText('Short');

		await page.getByRole('button', { name: 'Set aside stock' }).click();
		await expect(lines).toContainText('Ready');
		await page.getByRole('button', { name: 'Deliver what’s ready' }).click();
		await expect(lines.getByText('Delivered', { exact: true })).toBeVisible();
		await expect(lines).toContainText('For the agreement');

		await page.getByRole('button', { name: 'Make the agreement' }).click();
		const agreement = page.getByRole('dialog', { name: 'Make the agreement' });
		await agreement.getByRole('button', { name: 'Make it' }).click();
		await expect(page.getByText('Fulfilled', { exact: true })).toBeVisible();
		await expect(page.getByText(`Agreement: Kiosks ${stamp}`)).toBeVisible();
		await page.getByRole('button', { name: 'Close', exact: true }).click();
		await expect(page.getByText('Closed', { exact: true })).toBeVisible();
	});

	test('activities are planned and logged on a deal', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		for (const module of ['contracts', 'sales']) {
			await api.put(`/api/v1/modules/${module}`, { data: { enabled: true } });
		}
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = (await (
			await api.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Pied Piper ${stamp}` }
			})
		).json()) as { id: string };
		const opp = (await (
			await api.post('/api/v1/opportunities', {
				data: { orgId: client.id, opportunity: { name: `Compression ${stamp}` } }
			})
		).json()) as { id: string };
		await api.dispose();

		await signIn(page);
		await page.goto(`/sales/opportunities/${opp.id}`);
		await page.getByRole('button', { name: 'Log or plan something' }).click();
		await page.getByLabel('What').fill(`Send pricing ${stamp}`);
		await page.getByRole('button', { name: 'Add', exact: true }).click();
		const todo = page.getByRole('list', { name: 'Open tasks' });
		await expect(todo).toContainText(`Send pricing ${stamp}`);

		await page.getByRole('button', { name: 'Log or plan something' }).click();
		await page.getByLabel('Kind').selectOption('call');
		await page.getByLabel('What').fill('Intro call');
		await page.getByLabel('Minutes').fill('20');
		await page.getByRole('button', { name: 'Add', exact: true }).click();
		const history = page.getByRole('list', { name: 'Activity history' });
		await expect(history).toContainText('Intro call');
		await expect(history).toContainText('20 min');

		await openFromMenu(page, 'Activities');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('My activities');
		const item = page.getByRole('listitem').filter({ hasText: `Send pricing ${stamp}` });
		await item.getByRole('button', { name: 'Done' }).click();
		await expect(page.getByRole('list', { name: 'Done lately' })).toContainText(
			`Send pricing ${stamp}`
		);
	});
});
