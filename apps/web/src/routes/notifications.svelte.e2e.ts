import { expect, request, test } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const PASSWORD = 'notified e2e passphrase';

test.describe('notifications', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);

	test('the bell updates live when you are given a role', async ({ page }) => {
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const email = `notified-${Date.now().toString(36)}@example.com`;
		const created = await api.post('/api/v1/users', {
			data: { email, displayName: 'Noor Notified', password: PASSWORD }
		});
		const userId = ((await created.json()) as { user: { id: string } }).user.id;

		await page.goto('/login');
		await page.getByLabel('Email').fill(email);
		await page.getByLabel('Password').fill(PASSWORD);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await expect(page.getByTestId('unread-count')).toHaveCount(0);

		// Granted in the background: the open page learns about it without reloading.
		const roles = (await (await api.get('/api/v1/roles')).json()) as { id: string; key: string }[];
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const granted = await api.post('/api/v1/role-assignments', {
			data: {
				userId,
				roleId: roles.find((r) => r.key === 'technician')!.id,
				orgId: orgs.find((o) => o.parentId === null)!.id
			}
		});
		expect(granted.status()).toBe(201);
		await api.dispose();
		await expect(page.getByTestId('unread-count')).toHaveText('1', { timeout: 10000 });

		await page.getByRole('link', { name: 'Notifications, 1 unread' }).click();
		await expect(page.getByText('You were given the Technician role')).toBeVisible();
		await page
			.getByRole('button', { name: 'Mark “You were given the Technician role” read' })
			.click();
		await expect(page.getByTestId('unread-count')).toHaveCount(0);
		await expect(page.getByText('0 unread')).toBeVisible();
	});
});
