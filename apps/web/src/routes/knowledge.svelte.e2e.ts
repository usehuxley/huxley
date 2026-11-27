import { expect, request, test, type Page } from '@playwright/test';
import { openFromMenu } from '../lib/testing/nav.ts';

// Full-stack: needs the knowledge module on (the e2e job's service-desk
// preset). The owner writes, reviews and publishes.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const preset = process.env.E2E_PRESET ?? 'automation';
const apiUrl = process.env.HUXLEY_API_URL;

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

/** A ticket and two published articles at the root: one about the ticket. */
async function setUp(stamp: string) {
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
	const article = async (title: string, body: string) => {
		const res = await api.post('/api/v1/kb/articles', { data: { orgId: root, title, body } });
		expect(res.status(), await res.text()).toBe(201);
		const { id } = (await res.json()) as { id: string };
		expect((await api.post(`/api/v1/kb/articles/${id}/publish`, { data: {} })).status()).toBe(200);
		return id;
	};
	const fix = await article(`Unjam the ${stamp} plotter`, `Open the ${stamp} plotter tray.`);
	const other = await article(`Badge printer ${stamp}x`, 'Replace the ribbon.');
	const ticket = await api.post('/api/v1/tickets', {
		data: {
			orgId: root,
			title: `${stamp} plotter jammed`,
			description: `The ${stamp} plotter is stuck`
		}
	});
	expect(ticket.status(), await ticket.text()).toBe(201);
	const t = (await ticket.json()) as { id: string; reference: string };
	await api.dispose();
	return { fix, other, ticket: t };
}

test.describe('knowledge base', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');
	test.skip(preset === 'automation', 'the knowledge module is off in this run');

	test('an article is written, previewed, reviewed, published and found', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const title = `Reset MFA ${stamp}`;
		await signIn(page);
		await openFromMenu(page, 'Knowledge');
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Knowledge');
		await page.getByRole('link', { name: 'New article' }).click();

		await page.getByLabel('Title').fill(title);
		await page.getByLabel('Tags').fill('mfa, entra');
		await page
			.getByRole('textbox', { name: 'Text' })
			.fill(`1. Open **Entra**\n2. Find the user ${stamp}`);
		// The preview shows it as readers will, without running HTML.
		await page.getByRole('tab', { name: 'Preview' }).click();
		const preview = page.getByRole('document', { name: 'Preview' });
		await expect(preview.locator('strong')).toHaveText('Entra');
		await page.getByRole('tab', { name: 'Write' }).click();
		await page.getByRole('button', { name: 'Save draft' }).click();

		await expect(page.getByRole('heading', { level: 1 })).toHaveText(title);
		const review = page.getByTestId('kb-review');
		await expect(review).toContainText('A draft');
		await expect(page.getByRole('document', { name: 'Article' }).locator('strong')).toHaveText(
			'Entra'
		);

		// Submitted, then sent back with a reason.
		await review.getByRole('button', { name: 'Submit for review' }).click();
		await expect(review).toContainText('In review');
		await review.getByRole('button', { name: 'Reject' }).click();
		await page.getByLabel('What needs changing').fill('Add the PowerShell way');
		await page.getByRole('button', { name: 'Send back' }).click();
		await expect(page.getByRole('list', { name: 'History' })).toContainText(
			'Add the PowerShell way'
		);
		await expect(review).toContainText('A draft');

		// Fixed, resubmitted, approved.
		await page.getByRole('link', { name: 'Edit' }).click();
		await page
			.getByRole('textbox', { name: 'Text' })
			.fill(`1. Open **Entra**\n2. Find the user ${stamp}\n3. Or run \`Reset-Mfa\``);
		await page.getByRole('button', { name: 'Save', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(title);
		await review.getByRole('button', { name: 'Submit for review' }).click();
		await review.getByRole('button', { name: 'Approve and publish' }).click();
		await expect(review).toContainText('Published: readers see revision 2');

		// Every revision is kept.
		await page.getByRole('link', { name: 'Revisions' }).click();
		await expect(page.getByRole('list', { name: 'Revisions' }).getByRole('listitem')).toHaveCount(
			2
		);

		// Found by a word in its text, with the match shown.
		await page.goto(`/knowledge?q=${stamp}`);
		const found = page.getByRole('list', { name: 'Articles' }).getByRole('listitem');
		await expect(found).toHaveCount(1);
		await expect(found).toContainText(title);
		await expect(found.locator('b')).toContainText(stamp);
	});

	test('articles are suggested for a ticket, linked and unlinked', async ({ page }) => {
		test.skip(!apiUrl, 'set HUXLEY_API_URL');
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const { fix, ticket } = await setUp(stamp);
		await signIn(page);
		await page.goto(`/tickets/${ticket.id}`);
		const suggested = page.getByRole('list', { name: 'Suggested articles' });
		await expect(suggested).toContainText(`Unjam the ${stamp} plotter`);
		await suggested.getByRole('button', { name: `Link Unjam the ${stamp} plotter` }).click();
		const linked = page.getByRole('list', { name: 'Linked articles' });
		await expect(linked).toContainText(`Unjam the ${stamp} plotter`);
		await expect(page.getByRole('list', { name: 'Suggested articles' })).not.toContainText(
			`Unjam the ${stamp} plotter`
		);

		// Any article can be found and linked.
		await expect(async () => {
			await page.getByLabel('Find an article to link').fill(`Badge printer ${stamp}x`);
			await expect(page.getByRole('list', { name: 'Articles found' })).toBeVisible({
				timeout: 2000
			});
		}).toPass({ timeout: 15_000 });
		await page
			.getByRole('list', { name: 'Articles found' })
			.getByRole('button', { name: `Link Badge printer ${stamp}x` })
			.click();
		await expect(linked).toContainText(`Badge printer ${stamp}x`);

		// The article shows where it was used.
		await page.goto(`/knowledge/${fix}`);
		await expect(page.getByRole('list', { name: 'Linked tickets' })).toContainText(
			ticket.reference
		);

		// Unlinked from the ticket.
		await page.goto(`/tickets/${ticket.id}`);
		await page.getByRole('button', { name: `Unlink Badge printer ${stamp}x` }).click();
		await expect(page.getByRole('list', { name: 'Linked articles' })).not.toContainText(
			`Badge printer ${stamp}x`
		);
	});
});
