import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-055): an incident is linked to its problem, the problem
// becomes a known error, and its workaround is drafted into the knowledge
// base.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('problems', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a problem collects incidents and becomes a known error', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		await api.put('/api/v1/modules/knowledge', { data: { enabled: true } });
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const make = async (title: string, type?: string) =>
			(await (
				await api.post('/api/v1/tickets', {
					data: { orgId: root, title, ...(type ? { type } : {}) }
				})
			).json()) as {
				id: string;
				reference: string;
			};
		const problem = await make(`Wi-Fi drops ${stamp}`, 'problem');
		const incident = await make(`No Wi-Fi in room 2 ${stamp}`);
		await api.dispose();

		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await page.goto(`/tickets/${problem.id}`);
		await page.getByLabel('Ticket to link').fill(incident.reference);
		await page.getByLabel('Link as').selectOption('incident');
		await page.getByRole('button', { name: 'Link', exact: true }).click();
		await expect(page.getByText('1 incident linked')).toBeVisible();

		await page
			.getByLabel('Root cause', { exact: true })
			.fill('The access point firmware leaks memory.');
		await page.getByLabel('Workaround', { exact: true }).fill('Reboot the access point.');
		await page.getByLabel('Known error', { exact: false }).check();
		await page.getByRole('button', { name: 'Save problem' }).click();
		await expect(page.getByText('Problem saved.')).toBeVisible();
		await expect(page.getByText('Known error', { exact: true })).toBeVisible();
		await page
			.getByRole('button', { name: 'Draft a knowledge article from the workaround' })
			.click();
		await expect(page.getByText('Article drafted and sent for review.')).toBeVisible();
		await expect(page.getByRole('link', { name: 'The known-error article' })).toBeVisible();

		// The incident shows its problem.
		await page.goto(`/tickets/${incident.id}`);
		await expect(
			page
				.getByRole('listitem')
				.filter({ hasText: problem.reference })
				.filter({ hasText: 'Problem (its cause)' })
		).toHaveCount(1);
	});
});
