import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: with the customer experience module, a workflow at a new
// client scores its health; the client page shows the card (not enough
// data yet, every factor listed), and the weights can be changed.
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

test.describe('client health', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a client is scored and the weights are set', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		expect((await api.put('/api/v1/modules/cx', { data: { enabled: true } })).status()).toBe(200);
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const client = await api.post('/api/v1/orgs', {
			data: {
				parentId: orgs.find((o) => o.parentId === null)!.id,
				kind: 'client',
				name: `Health ${stamp}`
			}
		});
		const { id: clientId } = (await client.json()) as { id: string };
		const workflow = await api.post('/api/v1/workflows', {
			data: {
				orgId: clientId,
				document: {
					apiVersion: 'usehuxley.io/v1',
					kind: 'Workflow',
					metadata: { key: `health-${stamp}`, name: `Health ${stamp}` },
					spec: { steps: [{ id: 'score', type: 'core.health.compute' }] }
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

		await signIn(page);
		await page.goto(`/clients/${clientId}`);
		await expect(page.getByText('Not enough data')).toBeVisible();
		await page.getByText('All factors').click();
		const factors = page.getByRole('list', { name: 'Factors' });
		await expect(factors).toContainText('SLA targets met');
		await expect(factors).toContainText('too few to tell');

		await page.goto(`/settings/organizations/${clientId}`);
		await page.getByLabel('SLA targets met').fill('6');
		await page.getByRole('button', { name: 'Save weights' }).click();
		await expect(page.getByText('Weights saved.', { exact: false })).toBeVisible();
		await expect(page.getByLabel('SLA targets met')).toHaveValue('6');
	});
});
