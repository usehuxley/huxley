import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API (with a worker, so the workflow runs) and the
// servicedesk module on (the e2e job's service-desk preset).
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

/**
 * A "Reset MFA" Action for tickets, a workflow that answers with who it ran
 * for, and a client's ticket from Ada: [form name, ticket id, reference].
 */
async function setUp(stamp: string): Promise<[string, string, string]> {
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
	const name = `Reset MFA ${stamp}`;
	const key = `reset-mfa-${stamp}`;
	const form = await api.post('/api/v1/forms', {
		data: {
			orgId: root,
			form: {
				key,
				name,
				actionFor: 'ticket',
				schema: {
					type: 'object',
					properties: { reason: { type: 'string', title: 'Reason' } }
				}
			}
		}
	});
	expect(form.status(), await form.text()).toBe(201);
	const workflow = await api.post('/api/v1/workflows', {
		data: {
			orgId: root,
			document: {
				apiVersion: 'usehuxley.io/v1',
				kind: 'Workflow',
				metadata: { key: `wf-${key}`, name: `MFA ${stamp}` },
				spec: {
					triggers: [{ type: 'form', form: key }],
					steps: [{ id: 'who', type: 'transform', output: '{{ trigger.context.contact.email }}' }],
					outputs: { who: '{{ steps.who.output }}' }
				}
			}
		}
	});
	expect(workflow.status(), await workflow.text()).toBe(201);
	const wfId = ((await workflow.json()) as { id: string }).id;
	const published = await api.post(`/api/v1/workflows/${wfId}/publish`, { data: {} });
	expect(published.status(), await published.text()).toBe(200);
	const client = await api.post('/api/v1/orgs', {
		data: { parentId: root, kind: 'client', name: `Actions ${stamp}` }
	});
	const clientId = ((await client.json()) as { id: string }).id;
	const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
		data: { firstName: 'Ada', email: `ada-${stamp}@example.com` }
	});
	const contactId = ((await contact.json()) as { id: string }).id;
	const ticket = await api.post('/api/v1/tickets', {
		data: { orgId: clientId, title: `Lost phone ${stamp}`, requesterId: contactId }
	});
	const t = (await ticket.json()) as { id: string; reference: string };
	await api.dispose();
	return [name, t.id, t.reference];
}

test.describe('actions', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the servicedesk module is off in this run');

	test('an Action runs from a ticket, with the ticket and requester', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const [name, ticket, reference] = await setUp(stamp);
		await signIn(page);
		await page.goto(`/tickets/${ticket}`);
		await page.getByRole('list', { name: 'Actions' }).getByRole('button', { name }).click();

		// It opens beside the ticket, on the ticket and its requester.
		const drawer = page.getByRole('dialog', { name });
		await expect(drawer).toContainText(reference);
		await expect(drawer).toContainText('Ada');
		await drawer.getByLabel('Reason').fill('New phone');
		await drawer.getByRole('button', { name: 'Run' }).click();
		const runLink = drawer.getByRole('link', { name: /^Run [0-9a-f]{8}$/ });
		await expect(runLink).toBeVisible();
		const run = await runLink.getAttribute('href');

		// The ticket's history says so, without leaving the page.
		await drawer
			.getByRole('button', { name: /Done|Close/ })
			.last()
			.click();
		await expect(page.getByText(`ran ${name}`)).toBeVisible({ timeout: 20_000 });

		// The run had the requester: its output is Ada's email.
		await page.goto(run!);
		await expect(page.getByLabel('Run outputs')).toContainText(`ada-${stamp}@example.com`, {
			timeout: 20_000
		});
	});
});
