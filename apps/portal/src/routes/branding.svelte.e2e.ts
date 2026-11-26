import { expect, request, test } from '@playwright/test';

// Full-stack: the MSP's branding (set through the API) shows on the portal
// before anyone signs in. Needs the service-desk preset (portal on).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

const LOGO =
	'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==';

test.describe('portal branding', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('the sign-in page wears the MSP’s brand', async ({ page }) => {
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
		for (const [key, value] of [
			['branding.name', 'E2E Support'],
			['branding.accent_color', '#0b7285'],
			['branding.logo', LOGO]
		]) {
			const set = await api.put(`/api/v1/orgs/${root}/settings/${key}`, { data: { value } });
			expect(set.status(), await set.text()).toBe(204);
		}
		await api.dispose();

		await page.goto('/login');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Sign in to E2E Support');
		await expect(page.getByRole('img', { name: 'E2E Support' })).toBeVisible();
		await expect(page).toHaveTitle('Sign in · E2E Support');
	});
});
