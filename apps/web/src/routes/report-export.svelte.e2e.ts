import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-052): a dashboard downloads as a PDF, and "Email
// regularly" makes a published workflow that sends it.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('report export', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a dashboard downloads and is scheduled', async ({ page }) => {
		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);
		await page.goto('/dashboards/service-desk');

		await page.getByText('Download', { exact: true }).click();
		const downloading = page.waitForEvent('download');
		await page
			.getByRole('list', { name: 'Download as' })
			.getByRole('link', { name: 'PDF' })
			.click();
		const download = await downloading;
		expect(download.suggestedFilename()).toBe('service-desk-overview.pdf');
		const stream = await download.createReadStream();
		const chunks: Buffer[] = [];
		for await (const chunk of stream) chunks.push(chunk as Buffer);
		expect(Buffer.concat(chunks).subarray(0, 8).toString()).toBe('%PDF-1.4');

		await page.getByRole('button', { name: 'Email regularly' }).click();
		const dialog = page.getByRole('dialog', { name: 'Email this regularly' });
		await dialog.getByLabel('As').selectOption('xlsx');
		await dialog.getByRole('button', { name: 'Schedule' }).click();
		await expect(dialog.getByText('Scheduled. It runs as a workflow')).toBeVisible();
		await dialog.getByRole('link', { name: 'see or change it' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toContainText(
			'Email “Service desk overview”'
		);

		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const id = new URL(page.url()).pathname.split('/').pop()!;
		const workflow = (await (await api.get(`/api/v1/workflows/${id}`)).json()) as {
			currentVersion: number | null;
			draft: {
				spec: { triggers: { cron: string }[]; steps: { type: string; with: { format: string } }[] };
			};
		};
		expect(workflow.currentVersion).toBe(1);
		expect(workflow.draft.spec.triggers[0].cron).toBe('0 8 * * 1');
		expect(workflow.draft.spec.steps[0]).toMatchObject({
			type: 'core.report.send',
			with: { format: 'xlsx' }
		});
		await api.dispose();
	});
});
