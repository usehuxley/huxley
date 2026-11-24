import { expect, request, test, type APIRequestContext, type Page } from '@playwright/test';

// Full-stack dispatch: needs the API (with a worker, so the pack's workflow
// runs) and the servicedesk module on (the e2e job's service-desk preset).
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

async function owner(): Promise<APIRequestContext> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	return api;
}

/** Installs and publishes the built-in Service desk dispatch pack at the root. */
async function installPack(api: APIRequestContext): Promise<void> {
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const orgId = orgs.find((o) => o.parentId === null)!.id;
	const review = await api.post(`/api/v1/packs/builtin/huxley/servicedesk-dispatch?orgId=${orgId}`);
	expect(review.status()).toBe(201);
	const { uploadId, version, installedVersion } = (await review.json()) as {
		uploadId: string;
		version: string;
		installedVersion: string | null;
	};
	if (installedVersion === version) return;
	const installed = await api.post(`/api/v1/packs/uploads/${uploadId}/install`, {
		data: { orgId, version }
	});
	expect(installed.status(), await installed.text()).toBe(201);
	const { items } = (await installed.json()) as { items: { workflows: Record<string, string> } };
	for (const id of Object.values(items.workflows)) {
		const published = await api.post(`/api/v1/workflows/${id}/publish`, { data: {} });
		expect(published.status(), await published.text()).toBe(200);
	}
}

test.describe('dispatch', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('a round-robin queue assigns its new tickets, which show in My work', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const queue = `Dispatch ${stamp}`;
		const title = `Monitor flickers ${stamp}`;
		const api = await owner();
		await installPack(api);
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const client = `Dispatch client ${stamp}`;
		const created = await api.post('/api/v1/orgs', {
			data: { parentId: orgs.find((o) => o.parentId === null)!.id, kind: 'client', name: client }
		});
		expect(created.status()).toBe(201);
		await api.dispose();
		await signIn(page);

		// A queue that takes turns, with me in it.
		await page.goto('/settings/queues');
		await page.getByLabel('Name', { exact: true }).fill(queue);
		await page.getByLabel('Dispatch').selectOption('round_robin');
		await page.getByRole('checkbox', { name: 'E2E User' }).check();
		await page.getByRole('button', { name: 'Create queue' }).click();
		const row = page.getByRole('listitem').filter({ hasText: queue });
		await expect(row.getByText('Round-robin')).toBeVisible();

		// A ticket in it, unassigned: dispatch gives it to me.
		await page.goto('/tickets/new');
		await page.getByLabel('Client').selectOption({ label: client });
		await page.getByLabel('Title').fill(title);
		await page.getByLabel('Queue').selectOption({ label: queue });
		await page.getByRole('button', { name: 'Create ticket' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(title);
		await expect(async () => {
			await page.reload();
			await expect(page.getByLabel('Assignee').locator('option:checked')).toHaveText('E2E User', {
				timeout: 500
			});
		}).toPass({ timeout: 20_000 });

		// It's in My work, new, and the queue has nothing left waiting.
		await page.getByRole('link', { name: 'My work' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('My work');
		const mine = page.getByRole('list', { name: 'Assigned to me' });
		await expect(
			mine.getByRole('listitem').filter({ hasText: title }).getByText('New')
		).toBeVisible();
		await expect(page.getByText(queue, { exact: true })).toBeVisible();
		await expect(page.getByRole('list', { name: `Unassigned in ${queue}` })).toBeHidden();
	});
});
