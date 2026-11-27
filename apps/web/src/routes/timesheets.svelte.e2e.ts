import { expect, request, test, type Browser, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the API and the service-desk preset (workforce, tickets).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';
const PASSWORD = 'timesheet e2e passphrase';

async function signIn(browser: Browser, email: string, password: string): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
	return page;
}

/** Last week's Monday, as YYYY-MM-DD. */
function lastMonday(): string {
	const d = new Date();
	d.setUTCDate(d.getUTCDate() - 7);
	while (d.getUTCDay() !== 1) d.setUTCDate(d.getUTCDate() - 1);
	return d.toISOString().slice(0, 10);
}

/** A technician with an hour logged on a ticket last Monday: their email. */
async function setUp(stamp: string, monday: string): Promise<string> {
	const owner = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await owner.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const email = `tim-${stamp}@example.com`;
	const created = await owner.post('/api/v1/users', {
		data: { email, displayName: `Tim ${stamp}`, password: PASSWORD }
	});
	expect(created.status(), await created.text()).toBe(201);
	const userId = ((await created.json()) as { user: { id: string } }).user.id;
	const roles = (await (await owner.get('/api/v1/roles')).json()) as { id: string; key: string }[];
	const orgs = (await (await owner.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const root = orgs.find((o) => o.parentId === null)!.id;
	const granted = await owner.post('/api/v1/role-assignments', {
		data: { userId, roleId: roles.find((r) => r.key === 'technician')!.id, orgId: root }
	});
	expect(granted.status(), await granted.text()).toBe(201);
	const ticket = await owner.post('/api/v1/tickets', {
		data: { orgId: root, title: `Timesheet ${stamp}` }
	});
	const ticketId = ((await ticket.json()) as { id: string }).id;
	await owner.dispose();

	const tech = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await tech.post('/api/v1/auth/login', { data: { email, password: PASSWORD } });
	const types = (await (await tech.get('/api/v1/work-types')).json()) as {
		id: string;
		name: string;
	}[];
	const logged = await tech.post(`/api/v1/tickets/${ticketId}/time`, {
		data: {
			workTypeId: types.find((t) => t.name === 'Remote support')!.id,
			workedOn: monday,
			minutes: 60,
			billable: true,
			notes: 'Mailbox migration'
		}
	});
	expect(logged.status(), await logged.text()).toBe(201);
	await tech.dispose();
	return email;
}

test.describe('timesheets', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the workforce module is off in this run');

	test('a week is submitted, rejected with a comment, resubmitted and approved', async ({
		browser
	}) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const monday = lastMonday();
		const email = await setUp(stamp, monday);

		const tech = await signIn(browser, email, PASSWORD);
		await tech.goto(`/time?week=${monday}`);
		await expect(tech.getByText('Open', { exact: true })).toBeVisible();
		await tech.getByLabel('Note for your lead').fill('Migration week');
		await tech.getByRole('button', { name: 'Submit timesheet' }).click();
		await expect(tech.getByText('Timesheet submitted.')).toBeVisible();
		await expect(tech.getByRole('button', { name: 'Withdraw to change it' })).toBeVisible();

		// The workforce manager reviews it (Tim has no team lead).
		const owner = await signIn(browser, ownerEmail!, ownerPassword!);
		await openFromMenu(owner, 'Timesheets');
		await owner.getByRole('link', { name: `Tim ${stamp}` }).click();
		await expect(owner.getByRole('heading', { level: 1 })).toHaveText(`Tim ${stamp}’s timesheet`);
		await expect(owner.getByRole('list', { name: 'Entries' })).toContainText('Mailbox migration');
		await owner.getByLabel('Comment').fill('Add Tuesday’s call');
		await owner.getByRole('button', { name: 'Reject' }).click();
		await expect(owner.getByText('Rejected, with your comment.')).toBeVisible();

		await tech.reload();
		await expect(tech.getByRole('list', { name: 'Timesheet history' })).toContainText(
			'Add Tuesday’s call'
		);
		await tech.getByRole('button', { name: 'Resubmit' }).click();
		await expect(tech.getByText('Timesheet submitted.')).toBeVisible();

		await owner.reload();
		await owner.getByRole('button', { name: 'Approve' }).click();
		await expect(owner.getByText('Approved: its time is locked.')).toBeVisible();

		await tech.reload();
		await expect(
			tech.getByText('Approved: its time is locked. Your lead can reopen it.')
		).toBeVisible();
	});
});
