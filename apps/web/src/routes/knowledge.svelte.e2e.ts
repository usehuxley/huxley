import { expect, test, type Page } from '@playwright/test';

// Full-stack: needs the knowledge module on (the e2e job's service-desk
// preset). The owner writes, reviews and publishes.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const preset = process.env.E2E_PRESET ?? 'automation';

async function signIn(page: Page) {
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
}

test.describe('knowledge base', () => {
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');
	test.skip(preset === 'automation', 'the knowledge module is off in this run');

	test('an article is written, previewed, reviewed, published and found', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const title = `Reset MFA ${stamp}`;
		await signIn(page);
		await page.getByRole('link', { name: 'Knowledge' }).click();
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
});
