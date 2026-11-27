import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-048): a ticket in a queue with two members shows who
// should take it, with reasons; Assign gives it to them.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('dispatch recommendations', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a ticket shows who should take it', async ({ page }) => {
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
		const root = orgs.find((o) => o.parentId === null)!.id;
		const roles = (await (await api.get('/api/v1/roles')).json()) as { id: string; key: string }[];
		const technician = roles.find((r) => r.key === 'technician')!.id;
		const people: string[] = [];
		for (const name of ['Rhea', 'Sol']) {
			const created = await api.post('/api/v1/users', {
				data: {
					email: `${name.toLowerCase()}-${stamp}@example.com`,
					displayName: `${name} ${stamp}`,
					password: 'recommendations e2e passphrase'
				}
			});
			expect(created.status(), await created.text()).toBe(201);
			const id = ((await created.json()) as { user: { id: string } }).user.id;
			await api.post('/api/v1/role-assignments', {
				data: { userId: id, roleId: technician, orgId: root }
			});
			people.push(id);
		}
		const queue = await api.post('/api/v1/queues', {
			data: { orgId: root, name: `Desk ${stamp}`, userIds: people }
		});
		const { id: queueId } = (await queue.json()) as { id: string };
		// Sol already has a ticket open; Rhea is free.
		await api.post('/api/v1/tickets', {
			data: { orgId: root, title: `Busy ${stamp}`, queueId, assigneeUserId: people[1] }
		});
		const ticket = await api.post('/api/v1/tickets', {
			data: { orgId: root, title: `Who takes this ${stamp}`, queueId }
		});
		const { id } = (await ticket.json()) as { id: string };
		await api.dispose();

		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await page.goto(`/tickets/${id}`);
		const recommended = page.getByRole('list', { name: 'Recommended people' });
		await expect(recommended.getByRole('listitem').first()).toContainText(`Rhea ${stamp}`);
		await expect(recommended).toContainText('0 open tickets (the lightest load)');
		await expect(recommended).toContainText('1 open ticket');
		await recommended.getByRole('button', { name: `Assign to Rhea ${stamp}` }).click();
		await expect(page.getByText(`Assigned to Rhea ${stamp}.`)).toBeVisible();
		await expect(recommended).toContainText('Assigned now');
	});
});
