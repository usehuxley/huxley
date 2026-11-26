import { expect, request, test } from '@playwright/test';

// Full-stack with real email: a workflow at a client asks the net promoter
// question; a contact who raised a ticket is emailed 0–10 links, and the
// link's page records the score when they press Send. Needs the
// service-desk preset, MAILPIT_URL and the API's portal URL set to this
// preview server.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const mailpit = process.env.MAILPIT_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

async function staff() {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	return api;
}

/** The score link for `score` in the NPS email to `to`. */
async function scoreLink(to: string, score: number): Promise<URL> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 100; i++) {
			const query = encodeURIComponent(`to:"${to}" subject:"A quick question"`);
			const { messages } = (await (await api.get(`/api/v1/search?query=${query}`)).json()) as {
				messages: { ID: string }[];
			};
			if (messages.length > 0) {
				const message = (await (await api.get(`/api/v1/message/${messages[0].ID}`)).json()) as {
					Text: string;
				};
				const match = message.Text.match(
					new RegExp(`https?://\\S+/nps/\\S+\\?score=${score}(?!\\d)`)
				);
				if (match) return new URL(match[0]);
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no NPS email to ${to}`);
	} finally {
		await api.dispose();
	}
}

test.describe('net promoter surveys', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a contact answers the net promoter question from the email', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await staff();
		const on = await api.put('/api/v1/modules/cx', { data: { enabled: true } });
		expect(on.status(), await on.text()).toBe(200);
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		const client = await api.post('/api/v1/orgs', {
			data: { parentId: root, kind: 'client', name: `NPS ${stamp}` }
		});
		const { id: clientId } = (await client.json()) as { id: string };
		const email = `lou-${stamp}@example.com`;
		const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
			data: { firstName: 'Lou', lastName: stamp, email }
		});
		const { id: requesterId } = (await contact.json()) as { id: string };
		await api.post('/api/v1/tickets', {
			data: { orgId: clientId, title: `Help ${stamp}`, requesterId }
		});
		// A workflow at the client asks its people.
		const workflow = await api.post('/api/v1/workflows', {
			data: {
				orgId: clientId,
				document: {
					apiVersion: 'usehuxley.io/v1',
					kind: 'Workflow',
					metadata: { key: `nps-${stamp}`, name: `NPS ${stamp}` },
					spec: { steps: [{ id: 'ask', type: 'core.nps.send', with: { campaign: 'e2e' } }] }
				}
			}
		});
		expect(workflow.status(), await workflow.text()).toBe(201);
		const { id: workflowId } = (await workflow.json()) as { id: string };
		await api.post(`/api/v1/workflows/${workflowId}/publish`, { data: {} });
		const run = await api.post(`/api/v1/workflows/${workflowId}/runs`, { data: {} });
		expect(run.status(), await run.text()).toBe(201);

		const link = await scoreLink(email, 9);
		await page.goto(link.pathname + link.search);
		await expect(page.getByRole('heading', { level: 1 })).toContainText('A quick question from');
		await expect(page.getByRole('radio', { name: '9', exact: true })).toBeChecked();
		await page
			.getByLabel('What’s the main reason for your score? (optional)')
			.fill('Fast and friendly');
		await page.getByRole('button', { name: 'Send' }).click();
		await expect(page.getByText('Thanks for letting us know.')).toBeVisible();

		const nps = (await (await api.get(`/api/v1/contacts/${requesterId}/nps`)).json()) as {
			score: number;
			campaign: string;
		};
		expect([nps.score, nps.campaign]).toEqual([9, 'e2e']);
		await api.dispose();
	});
});
