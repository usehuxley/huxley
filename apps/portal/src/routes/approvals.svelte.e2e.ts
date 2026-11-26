import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack with real email: a workflow at a client asks the client's
// approvers; a client approver is emailed, approves in the portal, and the
// run carries on with the decision.
// Needs the service-desk preset, MAILPIT_URL and the API's portal URL set
// to this preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

/** The newest portal sign-in link emailed to `to`, other than `not`. */
async function linkIn(to: string, not?: string): Promise<string> {
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
				if (match && match[0] !== not) return match[0];
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no new email to ${to}`);
	} finally {
		await api.dispose();
	}
}

async function staff() {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	return api;
}

/** Whether an email with `subject` reached `to`. */
async function emailed(to: string, subject: string): Promise<boolean> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		const query = encodeURIComponent(`to:"${to}" subject:"${subject}"`);
		const { messages } = (await (await api.get(`/api/v1/search?query=${query}`)).json()) as {
			messages: unknown[];
		};
		return messages.length > 0;
	} finally {
		await api.dispose();
	}
}

/** A client, its approver (invited), and a published workflow there that asks for approval. */
async function setUp(api: APIRequestContext, stamp: string) {
	const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
		id: string;
		parentId: string | null;
	}[];
	const root = orgs.find((o) => o.parentId === null)!.id;
	const client = await api.post('/api/v1/orgs', {
		data: { parentId: root, kind: 'client', name: `Approvals ${stamp}` }
	});
	const { id: clientId } = (await client.json()) as { id: string };
	const email = `fay-${stamp}@example.com`;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Fay', lastName: stamp, email }
	});
	const { id } = (await contact.json()) as { id: string };
	const invited = await api.post(`/api/v1/contacts/${id}/portal-access`, {
		data: { role: 'client_approver' }
	});
	expect(invited.status(), await invited.text()).toBe(201);
	const workflow = await api.post('/api/v1/workflows', {
		data: {
			orgId: clientId,
			document: {
				apiVersion: 'usehuxley.io/v1',
				kind: 'Workflow',
				metadata: { key: `approve-${stamp}`, name: `Approve ${stamp}` },
				spec: {
					steps: [
						{
							id: 'ask',
							type: 'approval',
							title: `New laptop for Gus ${stamp}?`,
							message: 'A 14-inch laptop, about $1,400.',
							approvers: ['client_approvers'],
							expires: '1d'
						}
					],
					outputs: { decision: '{{ steps.ask.output.decision }}' }
				}
			}
		}
	});
	expect(workflow.status(), await workflow.text()).toBe(201);
	const { id: workflowId } = (await workflow.json()) as { id: string };
	expect((await api.post(`/api/v1/workflows/${workflowId}/publish`, { data: {} })).status()).toBe(
		200
	);
	return { email, workflowId };
}

test.describe('portal approvals', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the portal is off in this run');

	test('a client approver approves a request in the portal', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await staff();
		const { email, workflowId } = await setUp(api, stamp);
		const link = new URL(await linkIn(email));
		await page.goto(link.pathname + link.search);
		await page.getByRole('button', { name: 'Sign in' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('How can we help, Fay?');

		const started = await api.post(`/api/v1/workflows/${workflowId}/runs`, { data: {} });
		expect(started.status(), await started.text()).toBe(201);
		const { id: runId } = (await started.json()) as { id: string };
		// They're emailed, and it waits for them in the portal.
		await expect.poll(() => emailed(email, 'Approval needed'), { timeout: 20_000 }).toBe(true);
		await expect
			.poll(
				async () => {
					await page.goto('/');
					return page.getByText('One request is waiting for your approval.').isVisible();
				},
				{ timeout: 20_000 }
			)
			.toBe(true);
		await page.getByText('One request is waiting for your approval.').click();
		await page
			.getByRole('list', { name: 'Approvals' })
			.getByRole('link', { name: `New laptop for Gus ${stamp}?` })
			.click();
		await expect(page.getByText('A 14-inch laptop, about $1,400.')).toBeVisible();
		await page.getByLabel('Comment (optional)').fill('Fine by us');
		await page.getByRole('button', { name: 'Approve' }).click();
		await expect(page.getByText(`Approved by Fay ${stamp}`, { exact: false })).toBeVisible();

		await expect
			.poll(
				async () =>
					((await (await api.get(`/api/v1/runs/${runId}`)).json()) as { status: string }).status,
				{
					timeout: 20_000
				}
			)
			.toBe('succeeded');
		await api.dispose();
	});
});
