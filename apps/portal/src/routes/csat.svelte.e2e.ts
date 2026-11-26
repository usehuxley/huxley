import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack with real email: with the customer experience module and the
// built-in Satisfaction surveys pack, resolving a ticket emails its
// requester one-click ratings; the link opens a page that records the
// rating (with a comment) when they press Send. Needs the service-desk
// preset, MAILPIT_URL and the API's portal URL set to this preview server.
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

/** Turns the module on and installs (and publishes) the built-in pack, once. */
async function installCsat(api: APIRequestContext, orgId: string) {
	const on = await api.put('/api/v1/modules/cx', { data: { enabled: true } });
	expect(on.status(), await on.text()).toBe(200);
	const review = await api.post(`/api/v1/packs/builtin/huxley/csat?orgId=${orgId}`);
	expect(review.status(), await review.text()).toBe(201);
	const { uploadId, version, installedVersion } = (await review.json()) as {
		uploadId: string;
		version: string;
		installedVersion: string | null;
	};
	if (installedVersion === version) return;
	const installed = await api.post(`/api/v1/packs/uploads/${uploadId}/install`, {
		data: { orgId, version }
	});
	expect(installed.status(), await installed.text()).toBe(201);
	const { items } = (await installed.json()) as { items: { workflows: Record<string, string> } };
	for (const id of Object.values(items.workflows)) {
		const published = await api.post(`/api/v1/workflows/${id}/publish`, { data: {} });
		expect(published.status(), await published.text()).toBe(200);
	}
}

/** The rating link for `rating` in the survey email to `to`. */
async function ratingLink(to: string, rating: number): Promise<URL> {
	const api = await request.newContext({ baseURL: mailpit });
	try {
		for (let i = 0; i < 100; i++) {
			const query = encodeURIComponent(`to:"${to}" subject:"How did we do?"`);
			const { messages } = (await (await api.get(`/api/v1/search?query=${query}`)).json()) as {
				messages: { ID: string }[];
			};
			if (messages.length > 0) {
				const message = (await (await api.get(`/api/v1/message/${messages[0].ID}`)).json()) as {
					Text: string;
				};
				const match = message.Text.match(
					new RegExp(`https?://\\S+/survey/\\S+\\?rating=${rating}`)
				);
				if (match) return new URL(match[0]);
			}
			await new Promise((r) => setTimeout(r, 200));
		}
		throw new Error(`no survey email to ${to}`);
	} finally {
		await api.dispose();
	}
}

test.describe('satisfaction surveys', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl || !mailpit,
		'set E2E_EMAIL, E2E_PASSWORD, HUXLEY_API_URL and MAILPIT_URL'
	);
	test.skip(preset === 'automation', 'the service desk is off in this run');

	test('a requester rates a resolved ticket from the email', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const api = await staff();
		const orgs = (await (await api.get('/api/v1/orgs')).json()) as {
			id: string;
			parentId: string | null;
		}[];
		const root = orgs.find((o) => o.parentId === null)!.id;
		await installCsat(api, root);
		const client = await api.post('/api/v1/orgs', {
			data: { parentId: root, kind: 'client', name: `Survey ${stamp}` }
		});
		const { id: clientId } = (await client.json()) as { id: string };
		const email = `kim-${stamp}@example.com`;
		const contact = await api.post(`/api/v1/orgs/${clientId}/contacts`, {
			data: { firstName: 'Kim', lastName: stamp, email }
		});
		const { id: requesterId } = (await contact.json()) as { id: string };
		const created = await api.post('/api/v1/tickets', {
			data: { orgId: clientId, title: `Monitor flickers ${stamp}`, requesterId }
		});
		const ticket = (await created.json()) as { id: string; typeKey: string; version: number };
		const types = (await (await api.get('/api/v1/servicedesk/types')).json()) as {
			key: string;
			statuses: { key: string; category: string }[];
		}[];
		const resolved = types
			.find((t) => t.key === ticket.typeKey)!
			.statuses.find((s) => s.category === 'resolved')!.key;
		const done = await api.post(`/api/v1/tickets/${ticket.id}/status`, {
			data: { status: resolved, resolution: 'New cable', version: ticket.version }
		});
		expect(done.status(), await done.text()).toBe(200);

		// The email's "4 · Good" link opens the page with 4 chosen; Send records it.
		const link = await ratingLink(email, 4);
		await page.goto(link.pathname + link.search);
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(`Monitor flickers ${stamp}`);
		await expect(page.getByRole('radio', { name: '4 · Good' })).toBeChecked();
		await page.getByLabel('Anything to add? (optional)').fill('Sorted in a day');
		await page.getByRole('button', { name: 'Send' }).click();
		await expect(page.getByText('Thanks for letting us know.')).toBeVisible();

		const csat = (await (await api.get(`/api/v1/tickets/${ticket.id}/csat`)).json()) as {
			rating: number;
			channel: string;
			comment: string;
		};
		expect([csat.rating, csat.channel, csat.comment]).toEqual([4, 'email', 'Sorted in a day']);
		await api.dispose();
	});
});
