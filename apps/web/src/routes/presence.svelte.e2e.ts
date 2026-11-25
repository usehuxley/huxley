import { expect, request, test, type Browser, type Page } from '@playwright/test';

// Full-stack: needs the API and the servicedesk module on (the e2e job's
// service-desk preset).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';
const PASSWORD = 'presence e2e passphrase';

async function signIn(browser: Browser, email: string, password: string): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	return page;
}

/** A technician at the root, and a ticket: [tech's email, ticket id]. */
async function setUp(stamp: string): Promise<[string, string]> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const email = `tess-${stamp}@example.com`;
	const created = await api.post('/api/v1/users', {
		data: { email, displayName: `Tess ${stamp}`, password: PASSWORD }
	});
	const userId = ((await created.json()) as { user: { id: string } }).user.id;
	const roles = (await (await api.get('/api/v1/roles')).json()) as { id: string; key: string }[];
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const root = orgs.find((o) => o.parentId === null)!.id;
	const granted = await api.post('/api/v1/role-assignments', {
		data: { userId, roleId: roles.find((r) => r.key === 'technician')!.id, orgId: root }
	});
	expect(granted.status()).toBe(201);
	const ticket = await api.post('/api/v1/tickets', {
		data: { orgId: root, title: `Shared ${stamp}` }
	});
	expect(ticket.status()).toBe(201);
	const id = ((await ticket.json()) as { id: string }).id;
	await api.dispose();
	return [email, id];
}

test.describe('ticket presence', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('two people on a ticket see each other, and who is writing', async ({ browser }) => {
		const stamp = Date.now().toString(36);
		const [techEmail, ticket] = await setUp(stamp);
		const owner = await signIn(browser, ownerEmail!, ownerPassword!);
		await owner.goto(`/tickets/${ticket}`);
		const tech = await signIn(browser, techEmail, PASSWORD);
		await tech.goto(`/tickets/${ticket}`);
		await expect(tech.getByTestId('presence')).toHaveText('E2E User is also viewing');

		// The owner learns of the technician (on the next check-in or reload)…
		await expect(async () => {
			await owner.reload();
			await expect(owner.getByTestId('presence')).toHaveText(`Tess ${stamp} is also viewing`, {
				timeout: 2000
			});
		}).toPass({ timeout: 20_000 });

		// …and that they're writing.
		await tech.locator('#note-body').pressSequentially('On it');
		await expect(async () => {
			await owner.reload();
			await expect(owner.getByTestId('presence')).toHaveText(`Tess ${stamp} is writing`, {
				timeout: 2000
			});
		}).toPass({ timeout: 20_000 });

		// The technician leaves: the owner is alone again.
		await tech.context().close();
		await expect(async () => {
			await owner.reload();
			await expect(owner.getByTestId('presence')).toHaveCount(0, { timeout: 2000 });
		}).toPass({ timeout: 20_000 });
	});
});
