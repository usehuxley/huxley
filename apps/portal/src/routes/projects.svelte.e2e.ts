import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack (ADR-056, the Phase 9 exit criterion): a client onboarding
// project created from a template assigns its tasks across technicians,
// who see them in My Work; time on a task burns the budget and fires a
// threshold event (a workflow runs on it); a reached milestone shows in
// the client's portal. Needs the service-desk preset, MAILPIT_URL and the
// API's portal URL set to this preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';
const PASSWORD = 'projects e2e passphrase';

async function linkIn(to: string): Promise<string> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 50; i++) {
			const search = await api.get(`/api/v1/search?query=${encodeURIComponent(`to:"${to}"`)}`);
			const { messages } = (await search.json()) as { messages: { ID: string }[] };
			if (messages.length > 0) {
				const message = (await (await api.get(`/api/v1/message/${messages[0].ID}`)).json()) as {
					Text: string;
				};
				const match = message.Text.match(/https?:\/\/\S+\/auth\/link\?token=\S+/);
				if (match) return match[0];
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no email to ${to}`);
	} finally {
		await api.dispose();
	}
}

async function json<T>(
	r: Promise<import('@playwright/test').APIResponse>,
	status = 201
): Promise<T> {
	const res = await r;
	expect(res.status(), await res.text()).toBe(status);
	return (await res.json()) as T;
}

async function session(email: string, password: string): Promise<APIRequestContext> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	expect((await api.post('/api/v1/auth/login', { data: { email, password } })).status()).toBe(200);
	return api;
}

/** A technician at the root: id and a signed-in API session. */
async function technician(owner: APIRequestContext, name: string, stamp: string, root: string) {
	const email = `${name.toLowerCase()}-${stamp}@example.com`;
	const made = await json<{ user: { id: string } }>(
		owner.post('/api/v1/users', {
			data: { email, displayName: `${name} ${stamp}`, password: PASSWORD }
		})
	);
	const roles = await json<{ id: string; key: string }[]>(owner.get('/api/v1/roles'), 200);
	await json(
		owner.post('/api/v1/role-assignments', {
			data: {
				userId: made.user.id,
				roleId: roles.find((r) => r.key === 'technician')!.id,
				orgId: root
			}
		})
	);
	return { id: made.user.id, api: await session(email, PASSWORD) };
}

test.describe('projects', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('an onboarding project from a template, through to the portal', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const owner = await session(ownerEmail!, ownerPassword!);
		await json(owner.put('/api/v1/modules/projects', { data: { enabled: true } }), 200);
		const orgs = await json<{ id: string; parentId: string | null }[]>(
			owner.get('/api/v1/orgs'),
			200
		);
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = await json<{ id: string }>(
			owner.post('/api/v1/orgs', {
				data: { parentId: root, kind: 'client', name: `Onboard ${stamp}` }
			})
		);
		const ada = await technician(owner, 'Ada', stamp, root);
		const bob = await technician(owner, 'Bob', stamp, root);
		const template = await json<{ id: string }>(
			owner.post('/api/v1/project-templates', {
				data: {
					orgId: root,
					name: `Onboarding ${stamp}`,
					structure: {
						phases: [
							{
								name: 'Discovery',
								budgetHours: 2,
								tasks: [{ key: 'audit', title: `Audit ${stamp}`, estimateHours: 2 }]
							},
							{
								name: 'Rollout',
								budgetHours: 2,
								tasks: [{ key: 'deploy', title: `Deploy ${stamp}`, estimateHours: 2 }]
							}
						],
						milestones: [
							{ name: `Kickoff ${stamp}`, offsetDays: 0 },
							{ name: `Live ${stamp}`, offsetDays: 30 }
						]
					}
				}
			})
		);
		// A workflow at the client listens for budget thresholds.
		const listener = await json<{ id: string }>(
			owner.post('/api/v1/workflows', {
				data: {
					orgId: client.id,
					document: {
						apiVersion: 'usehuxley.io/v1',
						kind: 'Workflow',
						metadata: { key: `budget-${stamp}`, name: `Budget ${stamp}` },
						spec: {
							triggers: [{ type: 'event', event: 'project.budget_threshold' }],
							steps: [{ id: 'note', type: 'transform', output: '{{ inputs.threshold }}' }]
						}
					}
				}
			})
		);
		await json(owner.post(`/api/v1/workflows/${listener.id}/publish`, { data: {} }), 200);
		const project = await json<{
			id: string;
			tasks: { ticketId: string; title: string }[];
			milestones: { id: string }[];
		}>(
			owner.post('/api/v1/projects', {
				data: {
					orgId: client.id,
					name: `Onboarding ${stamp}`,
					startOn: new Date().toISOString().slice(0, 10),
					templateId: template.id,
					assignees: { audit: ada.id, deploy: bob.id }
				}
			})
		);
		// Each technician sees their task in My Work.
		for (const [who, title] of [
			[ada, `Audit ${stamp}`],
			[bob, `Deploy ${stamp}`]
		] as const) {
			const work = await json<{ mine: { title: string }[] }>(who.api.get('/api/v1/my-work'), 200);
			expect(work.mine.map((t) => t.title)).toContain(title);
		}
		// Ada logs three hours of four: 75% burned, thresholds fire.
		const audit = project.tasks.find((t) => t.title === `Audit ${stamp}`)!;
		const types = await json<{ id: string; onTickets: boolean }[]>(
			owner.get('/api/v1/work-types'),
			200
		);
		await json(
			ada.api.post(`/api/v1/tickets/${audit.ticketId}/time`, {
				data: { workTypeId: types.find((t) => t.onTickets)!.id, minutes: 180 }
			})
		);
		const burned = await json<{ burnPercent: number; thresholdsFired: number[] }>(
			owner.get(`/api/v1/projects/${project.id}`),
			200
		);
		expect(burned.burnPercent).toBe(75);
		expect(burned.thresholdsFired).toEqual([50, 75]);
		await expect
			.poll(
				async () =>
					(await json<unknown[]>(owner.get(`/api/v1/runs?workflowId=${listener.id}`), 200)).length,
				{
					timeout: 20_000
				}
			)
			.toBe(2);
		await json(
			owner.post(`/api/v1/projects/${project.id}/milestones/${project.milestones[0].id}`, {
				data: { reached: true }
			}),
			200
		);

		// A scope change goes to the client's approvers (the pack, at the client).
		const review = await json<{ uploadId: string; version: string }>(
			owner.post(`/api/v1/packs/builtin/huxley/project-scope-approvals?orgId=${client.id}`)
		);
		const installed = await json<{ items: { workflows: Record<string, string> } }>(
			owner.post(`/api/v1/packs/uploads/${review.uploadId}/install`, {
				data: { orgId: client.id, version: review.version }
			})
		);
		for (const id of Object.values(installed.items.workflows)) {
			await json(owner.post(`/api/v1/workflows/${id}/publish`, { data: {} }), 200);
		}
		const scope = await json<{ id: string }>(
			owner.post(`/api/v1/projects/${project.id}/scope-changes`, {
				data: { title: `Second site ${stamp}`, description: 'A second office.', hours: 2 }
			})
		);
		await json(owner.post(`/api/v1/projects/${project.id}/scope-changes/${scope.id}/submit`), 200);

		// The client's approver sees the milestones in the portal.
		const email = `cleo-${stamp}@example.com`;
		const contact = await json<{ id: string }>(
			owner.post(`/api/v1/orgs/${client.id}/contacts`, {
				data: { firstName: 'Cleo', lastName: stamp, email }
			})
		);
		await json(
			owner.post(`/api/v1/contacts/${contact.id}/portal-access`, {
				data: { role: 'client_approver' }
			})
		);
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await page.getByRole('link', { name: 'Projects', exact: true }).click();
		const milestones = page.getByRole('list', { name: `Milestones of Onboarding ${stamp}` });
		await expect(
			milestones.getByRole('listitem').filter({ hasText: `Kickoff ${stamp}` })
		).toContainText('Reached');
		await expect(
			milestones.getByRole('listitem').filter({ hasText: `Live ${stamp}` })
		).toContainText('Upcoming');
		// ...and approves the scope change, which grows the budget.
		const title = `Approve scope change 1 on Onboarding ${stamp}: Second site ${stamp}`;
		await expect
			.poll(
				async () => {
					await page.goto('/approvals');
					return page.getByRole('link', { name: title }).isVisible();
				},
				{ timeout: 20_000 }
			)
			.toBe(true);
		await page.getByRole('link', { name: title }).click();
		await expect(page.getByText('It changes the work by 2', { exact: false })).toBeVisible();
		await page.getByRole('button', { name: 'Approve' }).click();
		await expect
			.poll(
				async () =>
					(await json<{ budgetHours: number }>(owner.get(`/api/v1/projects/${project.id}`), 200))
						.budgetHours,
				{
					timeout: 20_000
				}
			)
			.toBe(6);
		await page.goto('/projects');
		await expect(
			page.getByRole('list', { name: `Scope changes of Onboarding ${stamp}` }).getByRole('listitem')
		).toContainText('Approved');
		for (const c of [owner, ada.api, bob.api]) await c.dispose();
	});
});
