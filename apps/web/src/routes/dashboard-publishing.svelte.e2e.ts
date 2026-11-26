import { expect, request, test, type APIRequestContext, type Browser } from '@playwright/test';

// Full-stack (design doc §8.12, the Phase 8 exit criterion): a technician
// builds a dashboard and submits it for their team and the client portal;
// the owner reviews and approves it; then a teammate sees it.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const PASSWORD = 'publishing e2e passphrase';

async function session(email: string, password: string): Promise<APIRequestContext> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	expect((await api.post('/api/v1/auth/login', { data: { email, password } })).status()).toBe(200);
	return api;
}

/** A technician at the root: their email and id. */
async function technician(owner: APIRequestContext, name: string, stamp: string, root: string) {
	const email = `${name.toLowerCase()}-${stamp}@example.com`;
	const created = await owner.post('/api/v1/users', {
		data: { email, displayName: `${name} ${stamp}`, password: PASSWORD }
	});
	expect(created.status(), await created.text()).toBe(201);
	const userId = ((await created.json()) as { user: { id: string } }).user.id;
	const roles = (await (await owner.get('/api/v1/roles')).json()) as { id: string; key: string }[];
	const tech = roles.find((r) => r.key === 'technician')!;
	await owner.post('/api/v1/role-assignments', { data: { userId, roleId: tech.id, orgId: root } });
	return { email, userId };
}

async function signIn(browser: Browser, email: string, password: string) {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	return page;
}

test.describe('dashboard publishing', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);

	test('a dashboard is submitted, approved, and shared with a team', async ({ browser }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const owner = await session(ownerEmail!, ownerPassword!);
		const orgs = (await (await owner.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const author = await technician(owner, 'Avery', stamp, root);
		const mate = await technician(owner, 'Morgan', stamp, root);
		const team = await owner.post('/api/v1/teams', { data: { name: `Ops ${stamp}` } });
		expect(team.status(), await team.text()).toBe(201);
		const teamId = ((await team.json()) as { id: string }).id;
		await owner.put(`/api/v1/teams/${teamId}/members/${mate.userId}`, { data: {} });
		await owner.dispose();

		// The author builds a personal dashboard (the editor has its own test).
		const authorApi = await session(author.email, PASSWORD);
		const built = await authorApi.post('/api/v1/dashboards', {
			data: {
				orgId: root,
				dashboard: {
					name: `Ops board ${stamp}`,
					widgets: [
						{
							id: 'open',
							title: 'Tickets raised',
							viz: 'number',
							width: 4,
							spec: { source: 'runs', measures: [{ fn: 'count' }] }
						}
					]
				}
			}
		});
		expect(built.status(), await built.text()).toBe(201);
		const { id } = (await built.json()) as { id: string };
		await authorApi.dispose();

		const authorPage = await signIn(browser, author.email, PASSWORD);
		await authorPage.goto(`/dashboards/${id}`);
		await authorPage.getByRole('button', { name: 'Publish' }).click();
		const dialog = authorPage.getByRole('dialog', { name: 'Submit for publishing' });
		await dialog.getByLabel(`Ops ${stamp}`).check();
		await dialog.getByLabel('Note for the reviewer').fill('For the ops stand-up');
		await dialog.getByRole('button', { name: 'Submit' }).click();
		await expect(authorPage.getByText('Waiting for approval since')).toBeVisible();

		// The owner reviews it from the dashboards list and approves it.
		const ownerPage = await signIn(browser, ownerEmail!, ownerPassword!);
		await ownerPage.goto('/dashboards');
		await ownerPage
			.getByRole('list', { name: 'Waiting for your approval' })
			.getByRole('link', { name: `Ops board ${stamp}` })
			.click();
		await expect(ownerPage.getByText('For the ops stand-up')).toBeVisible();
		await expect(ownerPage.getByText(`Ops ${stamp}`)).toBeVisible();
		await ownerPage.getByRole('button', { name: 'Approve and publish' }).click();
		await expect(ownerPage).toHaveURL(new RegExp(`/dashboards/${id}$`));
		await expect(ownerPage.getByText(`Shared with Ops ${stamp}`)).toBeVisible();

		// The teammate sees it among the shared dashboards.
		const matePage = await signIn(browser, mate.email, PASSWORD);
		await matePage.goto('/dashboards');
		await expect(
			matePage.getByRole('list', { name: 'Shared dashboards' }).getByText(`Ops board ${stamp}`)
		).toBeVisible();
	});
});
