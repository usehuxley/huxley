import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack (ADR-054, the Phase 9 exit criterion): a high-risk normal
// change shows its impacted assets, is refused in a freeze window, then
// goes to the CAB and, approved there, to the client's approver, who
// approves it in the portal. Needs the service-desk preset, MAILPIT_URL
// and the API's portal URL set to this preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

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

/** A client with a server and an app on it, its approver (invited), and the pack there. */
async function setUp(api: APIRequestContext, stamp: string) {
	const me = await json<{ id: string }>(api.get('/api/v1/auth/me'), 200);
	const orgs = await json<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'), 200);
	const client = await json<{ id: string }>(
		api.post('/api/v1/orgs', {
			data: {
				parentId: orgs.find((o) => o.parentId === null)!.id,
				kind: 'client',
				name: `Changes ${stamp}`
			}
		})
	);
	// The CAB is the Administrator role: the owner holds it here.
	const roles = await json<{ id: string; key: string }[]>(api.get('/api/v1/roles'), 200);
	await json(
		api.post('/api/v1/role-assignments', {
			data: { userId: me.id, roleId: roles.find((r) => r.key === 'admin')!.id, orgId: client.id }
		})
	);
	const types = await json<{ id: string }[]>(api.get('/api/v1/asset-types'), 200);
	const server = await json<{ id: string }>(
		api.post('/api/v1/assets', {
			data: { orgId: client.id, typeId: types[0].id, name: `SRV ${stamp}` }
		})
	);
	const app = await json<{ id: string }>(
		api.post('/api/v1/assets', {
			data: { orgId: client.id, typeId: types[0].id, name: `Payroll ${stamp}` }
		})
	);
	const related = await api.post(`/api/v1/assets/${app.id}/relationships`, {
		data: { kind: 'runs_on', toId: server.id }
	});
	expect(related.ok(), await related.text()).toBe(true);
	const email = `cora-${stamp}@example.com`;
	const contact = await json<{ id: string }>(
		api.post(`/api/v1/orgs/${client.id}/contacts`, {
			data: { firstName: 'Cora', lastName: stamp, email }
		})
	);
	await json(
		api.post(`/api/v1/contacts/${contact.id}/portal-access`, { data: { role: 'client_approver' } })
	);
	const review = await json<{ uploadId: string; version: string }>(
		api.post(`/api/v1/packs/builtin/huxley/change-approvals?orgId=${client.id}`)
	);
	const installed = await json<{ items: { workflows: Record<string, string> } }>(
		api.post(`/api/v1/packs/uploads/${review.uploadId}/install`, {
			data: { orgId: client.id, version: review.version }
		})
	);
	for (const id of Object.values(installed.items.workflows)) {
		await json(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }), 200);
	}
	return { clientId: client.id, server: server.id, app: app.id, email };
}

test.describe('change approvals', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a high-risk change goes to the CAB, then the client approves it in the portal', async ({
		page
	}) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const { clientId, server, app, email } = await setUp(api, stamp);
		const ticket = await json<{ id: string; reference: string }>(
			api.post('/api/v1/tickets', {
				data: { orgId: clientId, type: 'change', title: `Upgrade SRV ${stamp}` }
			})
		);
		const change = `/api/v1/tickets/${ticket.id}/change`;
		const day = 86_400_000;
		const at = (offset: number) => new Date(Date.now() + offset).toISOString();
		const freeze = await json<{ id: string }>(
			api.post('/api/v1/change-freezes', {
				data: {
					orgId: clientId,
					name: `Quarter end ${stamp}`,
					startsAt: at(9 * day),
					endsAt: at(11 * day)
				}
			})
		);
		await json(
			api.put(change, {
				data: {
					riskAnswers: {
						reach: 'everyone',
						downtime: 'hours',
						tested: 'partly',
						backout: 'hard',
						familiar: 'once'
					},
					implementationPlan: 'Upgrade and reboot.',
					backoutPlan: 'Restore the snapshot.',
					plannedStart: at(10 * day),
					plannedEnd: at(10 * day + 7_200_000),
					affectedAssets: [server]
				}
			}),
			200
		);
		// It shows what else is impacted, and the freeze holds it back.
		const impact = await json<{ impacted: { id: string }[] }>(api.get(`${change}/impact`), 200);
		expect(impact.impacted.map((i) => i.id)).toContain(app);
		const refused = await api.post(`${change}/submit`);
		expect(refused.status()).toBe(422);
		expect(await refused.text()).toContain(`Quarter end ${stamp}`);
		await json(
			api.put(change, {
				data: { plannedStart: at(14 * day), plannedEnd: at(14 * day + 7_200_000) }
			}),
			200
		);
		await json(api.post(`${change}/submit`), 200);
		await api.delete(`/api/v1/change-freezes/${freeze.id}`);

		// The CAB (the owner, an administrator there) approves.
		const title = `Approve change ${ticket.reference}: Upgrade SRV ${stamp}`;
		let cabId = '';
		await expect
			.poll(
				async () => {
					const mine = await json<{ id: string; title: string }[]>(
						api.get('/api/v1/approvals'),
						200
					);
					cabId = mine.find((a) => a.title === title)?.id ?? '';
					return cabId;
				},
				{ timeout: 20_000 }
			)
			.not.toBe('');
		await json(
			api.post(`/api/v1/approvals/${cabId}/decision`, { data: { decision: 'approve' } }),
			200
		);

		// Then the client's approver, in the portal.
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
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
		await expect(
			page.getByText('Your IT provider plans a high-risk change', { exact: false })
		).toBeVisible();
		await page.getByRole('button', { name: 'Approve' }).click();
		await expect(page.getByText(`Approved by Cora ${stamp}`, { exact: false })).toBeVisible();

		await expect
			.poll(async () => (await json<{ state: string }>(api.get(change), 200)).state, {
				timeout: 20_000
			})
			.toBe('approved');
		const approved = await json<{ decidedBy: string }>(api.get(change), 200);
		expect(approved.decidedBy).toBe(`Cora ${stamp} (client)`);
		await api.dispose();
	});
});
