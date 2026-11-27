import { expect, request, test } from '@playwright/test';

// Full-stack (ADR-056): a project template is built in Settings, a project
// is created from it with a task assigned, and a milestone is reached.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

test.describe('projects', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a project from a template built in Settings', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		await api.put('/api/v1/modules/projects', { data: { enabled: true } });
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = `Prj ${stamp}`;
		await api.post('/api/v1/orgs', { data: { parentId: root, kind: 'client', name: client } });
		await api.dispose();

		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page).toHaveURL('/');

		// The template: one phase with a task, and a milestone.
		await page.goto('/settings/project-templates');
		await page.getByRole('link', { name: 'New template' }).click();
		await page.getByLabel('Template name').fill(`Rollout ${stamp}`);
		const phase = page.getByRole('group', { name: 'Phase 1' });
		await phase.getByLabel('Phase name').fill('Install');
		await phase.getByLabel('Budget hours').fill('8');
		await phase.getByRole('button', { name: 'Add a task' }).click();
		await phase.getByLabel('Task', { exact: true }).fill(`Install agents ${stamp}`);
		await phase.getByLabel('Hours', { exact: true }).fill('8');
		await page.getByRole('button', { name: 'Add a milestone' }).click();
		await page.getByLabel('Milestone', { exact: true }).fill(`Go live ${stamp}`);
		await page.getByLabel('Days after start').fill('7');
		await page.getByRole('button', { name: 'Save template' }).click();
		await expect(page.getByRole('link', { name: `Rollout ${stamp}` })).toBeVisible();

		// The project, with the task assigned to the owner.
		await page.goto('/projects/new');
		await page.getByLabel('Client').selectOption({ label: client });
		await page.getByLabel('Name', { exact: true }).fill(`Rollout for ${stamp}`);
		await page.getByLabel('Template').selectOption({ label: `Rollout ${stamp}` });
		await page.getByLabel(`Install agents ${stamp}`).selectOption({ label: 'E2E User' });
		await page.getByRole('button', { name: 'Create project' }).click();
		await expect(page.getByRole('heading', { name: `Rollout for ${stamp}` })).toBeVisible();
		await expect(page.getByLabel('Budget used')).toHaveText('0 of 8 hours');
		const tasks = page.getByRole('list', { name: 'Install tasks' });
		await expect(tasks.getByRole('listitem')).toContainText(`Install agents ${stamp}`);
		await expect(tasks.getByRole('listitem')).toContainText('E2E User');
		await page.getByRole('button', { name: `Mark Go live ${stamp} reached` }).click();
		await expect(
			page.getByRole('list', { name: 'Milestones' }).getByRole('listitem')
		).toContainText('Reached');
		// Dates on the task put it on the timeline; the board shows it to do.
		await tasks.getByText(/^Edit /).click();
		await tasks.getByLabel('Starts').fill('2030-01-07');
		await tasks.getByLabel('Due').fill('2030-01-09');
		await tasks.getByRole('button', { name: 'Save task' }).click();
		await expect(tasks.getByRole('listitem')).toContainText('2030-01-07 → 2030-01-09');
		await page.getByRole('link', { name: 'Board', exact: true }).click();
		await expect(page.getByRole('region', { name: 'To do column' })).toContainText(
			`Install agents ${stamp}`
		);
		await page.getByRole('link', { name: 'Timeline', exact: true }).click();
		await expect(
			page
				.getByRole('figure', { name: 'Timeline' })
				.getByRole('link', { name: `Install agents ${stamp}` })
		).toBeVisible();

		// The calendar: the task starts on the 7th and is due on the 9th.
		await page.getByRole('link', { name: 'Calendar', exact: true }).click();
		await page.getByRole('link', { name: 'Next month' }).waitFor();
		await page.goto(`${page.url().split('?')[0]}?view=calendar&month=2030-01`);
		await expect(page.getByRole('list', { name: 'On 2030-01-07' })).toContainText(
			`Install agents ${stamp}`
		);
		await expect(page.getByRole('list', { name: 'On 2030-01-09' })).toContainText(
			`Due: Install agents ${stamp}`
		);

		// A scope change, drafted and submitted.
		await page.getByLabel('Change', { exact: true }).fill(`Extra site ${stamp}`);
		await page.getByLabel('Hours (+/−)').fill('4');
		await page.getByRole('button', { name: 'Draft scope change' }).click();
		const scope = page.getByRole('list', { name: 'Scope changes' });
		await expect(scope.getByRole('listitem')).toContainText('Draft');
		await page.getByRole('button', { name: 'Submit scope change 1' }).click();
		await expect(scope.getByRole('listitem')).toContainText('Waiting for the client');

		// A status report, written and shared.
		await page.getByLabel('Write a report').fill(`Agents going in ${stamp}.`);
		await page.getByRole('button', { name: 'Save report' }).click();
		const report = page.getByRole('article', { name: /^Status report/ });
		await expect(report).toContainText(`Agents going in ${stamp}.`);
		await report.getByRole('button', { name: 'Share with the client' }).click();
		await expect(report).toContainText('Shared with the client');

		await page.goto('/projects');
		await expect(page.getByRole('link', { name: `Rollout for ${stamp}` })).toBeVisible();

		// Capacity: the task's 8 hours fall in the week of 7 January 2030.
		await page.getByRole('link', { name: 'Capacity', exact: true }).click();
		await page.goto('/projects/capacity?from=2030-01-07');
		const row = page
			.getByRole('row')
			.filter({ has: page.getByRole('rowheader', { name: /E2E User/ }) });
		await expect(row.getByRole('cell').first()).toContainText('8 project');
	});
});
