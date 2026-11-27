import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-051): a workflow suggests time on your ticket; My time
// shows it, and logging it makes a time entry.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('suggested time', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('suggested time is logged from My time', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const me = ((await (await api.get('/api/v1/auth/me')).json()) as { id: string }).id;
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const ticket = await api.post('/api/v1/tickets', {
			data: { orgId: root, title: `Backups failing ${stamp}`, assigneeUserId: me }
		});
		const { id: ticketId } = (await ticket.json()) as { id: string };
		const workflow = await api.post('/api/v1/workflows', {
			data: {
				orgId: root,
				document: {
					apiVersion: 'usehuxley.io/v1',
					kind: 'Workflow',
					metadata: { key: `suggest-${stamp}`, name: `Suggest ${stamp}` },
					spec: {
						steps: [
							{
								id: 'time',
								type: 'core.time.suggest',
								with: { ticket_id: ticketId, minutes: 35, notes: `Fixed the backup job ${stamp}` }
							}
						]
					}
				}
			}
		});
		expect(workflow.status(), await workflow.text()).toBe(201);
		const { id: workflowId } = (await workflow.json()) as { id: string };
		await api.post(`/api/v1/workflows/${workflowId}/publish`, { data: {} });
		const run = (await (
			await api.post(`/api/v1/workflows/${workflowId}/runs`, { data: {} })
		).json()) as {
			id: string;
		};
		await expect
			.poll(
				async () =>
					((await (await api.get(`/api/v1/runs/${run.id}`)).json()) as { status: string }).status,
				{
					timeout: 20_000
				}
			)
			.toBe('succeeded');
		await api.dispose();

		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await page.goto('/time');
		const suggested = page
			.getByRole('list', { name: 'Suggested time' })
			.getByRole('listitem')
			.filter({ hasText: `Backups failing ${stamp}` });
		await expect(suggested).toContainText(`Fixed the backup job ${stamp}`);
		await expect(suggested.getByLabel('Minutes')).toHaveValue('35');
		await suggested.getByLabel('Minutes').fill('40');
		await suggested.getByRole('button', { name: 'Log it' }).click();
		await expect(page.getByText(`Fixed the backup job ${stamp}`).first()).toBeVisible();
		await expect(
			page
				.getByRole('list', { name: 'Suggested time' })
				.filter({ hasText: `Backups failing ${stamp}` })
		).toHaveCount(0);
	});
});
