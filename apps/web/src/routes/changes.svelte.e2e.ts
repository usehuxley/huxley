import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-054): a change is filled in on its ticket (type, risk,
// plans, window), submitted, and shows on the change calendar.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('changes', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a change is assessed, submitted and on the calendar', async ({ page }) => {
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
		const ticket = (await (
			await api.post('/api/v1/tickets', {
				data: {
					orgId: orgs.find((o) => o.parentId === null)!.id,
					type: 'change',
					title: `Rotate certs ${stamp}`
				}
			})
		).json()) as { id: string };
		await api.dispose();

		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await page.goto(`/tickets/${ticket.id}`);
		await page.locator('html[data-hydrated]').waitFor({ state: 'attached' });
		const risk = page.getByRole('group', { name: 'Risk' });
		for (const [question, answer] of [
			['Who would notice if it went wrong?', 'A few people'],
			['How much downtime does it need?', 'Minutes'],
			['Has it been tested?', 'Yes, fully'],
			['How hard is it to back out?', 'Easy'],
			['Have you done this before?', 'Often']
		]) {
			await risk.getByLabel(question).selectOption({ label: answer });
		}
		await page.getByLabel('Implementation plan').fill('Rotate the certificates.');
		await page.getByLabel('Backout plan').fill('Restore the old ones.');
		const start = new Date(Date.now() + 3 * 86_400_000);
		start.setHours(9, 0, 0, 0);
		const local = (d: Date) => {
			const pad = (n: number) => String(n).padStart(2, '0');
			return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
		};
		await page.getByLabel('Window starts').fill(local(start));
		await page.getByLabel('Window ends').fill(local(new Date(start.getTime() + 3_600_000)));
		await page.getByRole('button', { name: 'Save change' }).click();
		await expect(page.getByText('Change saved.')).toBeVisible();
		await expect(page.getByText('low risk (2)')).toBeVisible();
		await page.getByRole('button', { name: 'Submit for approval' }).click();
		await expect(page.getByText('Submitted for approval.')).toBeVisible();
		await expect(page.getByText('Waiting for approval')).toBeVisible();

		await page.goto('/changes');
		await expect(page.getByRole('list', { name: 'Change calendar' })).toContainText(
			`Rotate certs ${stamp}`
		);
	});
});
