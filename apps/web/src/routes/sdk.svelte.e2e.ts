import { execFileSync } from 'node:child_process';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: a workflow written with @usehuxley/sdk (the example timesheet
// reminders) is built to HWF, saved through the API as Git sync would, and
// opens on the canvas with its nested steps. Needs the service-desk preset
// (the workforce module's step).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';
const root = fileURLToPath(new URL('../../../../', import.meta.url));

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
}

/** Builds the SDK examples: the built workflow named `key`. */
function built(key: string): Record<string, unknown> {
	const out = mkdtempSync(join(tmpdir(), 'sdk-e2e-'));
	try {
		execFileSync(
			process.execPath,
			[join(root, 'packages/sdk/src/cli.ts'), 'build', join(root, 'examples/sdk'), '--out', out],
			{ cwd: join(root, 'examples/sdk'), stdio: 'pipe' }
		);
		return JSON.parse(
			readFileSync(join(out, 'timesheet-reminders', `${key}.json`), 'utf8')
		) as Record<string, unknown>;
	} finally {
		rmSync(out, { recursive: true, force: true });
	}
}

test.describe('the TypeScript SDK', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the workforce module is off in this run');

	test('a workflow built with the SDK opens on the canvas', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const document = built('timesheet-unsubmitted');
		const metadata = document.metadata as Record<string, unknown>;
		metadata.key = `sdk-unsubmitted-${stamp}`;
		metadata.name = `SDK reminders ${stamp}`;

		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const created = await api.post('/api/v1/workflows', {
			data: { orgId: orgs.find((o) => o.parentId === null)!.id, document }
		});
		expect(created.status(), await created.text()).toBe(201);
		const { id } = (await created.json()) as { id: string };
		await api.dispose();

		await signIn(page);
		await page.goto(`/workflows/${id}`);
		await expect(page.getByRole('heading', { level: 1 })).toContainText(`SDK reminders ${stamp}`);
		for (const step of ['find', 'remind', 'tell', 'late', 'tell_leads']) {
			await expect(page.getByRole('button', { name: new RegExp(`^Step ${step},`) })).toBeAttached();
		}
		await page.getByRole('tab', { name: 'YAML' }).click();
		await expect(page.getByRole('textbox', { name: 'Workflow YAML' })).toContainText(
			'lead_after_days'
		);
	});
});
