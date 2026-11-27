import { expect, request, test, type Browser, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL.
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const PASSWORD = 'team e2e passphrase';

async function signIn(browser: Browser, email: string, password: string): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(email);
	await page.getByLabel('Password').fill(password);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText(
		/^Good (morning|afternoon|evening), /
	);
	return page;
}

test.describe('teams', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);

	test("members get the team's role and lose it when removed", async ({ browser }) => {
		const suffix = Date.now().toString(36);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
		const memberEmail = `teammate-${suffix}@example.com`;
		const memberName = `Terry ${suffix}`;
		expect(
			(
				await api.post('/api/v1/users', {
					data: { email: memberEmail, displayName: memberName, password: PASSWORD }
				})
			).status()
		).toBe(201);
		await api.dispose();

		// The owner builds a team with the Technician role at the root. (Not
		// Read-only: the MFA policy test changes that role's policy meanwhile.)
		const owner = await signIn(browser, ownerEmail!, ownerPassword!);
		await owner.goto('/settings/teams');
		const teamName = `Auditors ${suffix}`;
		await owner.getByLabel('Team name').fill(teamName);
		await owner.getByRole('button', { name: 'Create team' }).click();
		await expect(owner.getByRole('heading', { name: teamName })).toBeVisible();
		await owner.getByLabel('Add member').selectOption({ label: `${memberName} (${memberEmail})` });
		await owner.getByRole('button', { name: 'Add', exact: true }).click();
		await expect(owner.getByRole('button', { name: `Remove ${memberName}` })).toBeVisible();
		await owner.getByLabel('Role', { exact: true }).selectOption({ label: 'Technician' });
		await owner.getByLabel('At', { exact: true }).selectOption({ index: 1 });
		await owner.getByRole('button', { name: 'Assign role' }).click();
		await expect(owner.getByText('Role assigned.')).toBeVisible();

		// Through the team, the member can see users.
		const member = await signIn(browser, memberEmail, PASSWORD);
		await member.goto('/settings/users');
		await expect(member).toHaveURL(/\/settings\/users$/);
		await expect(member.getByText(memberEmail)).toBeVisible();

		// Removed from the team: access goes at the next request.
		await owner.getByRole('button', { name: `Remove ${memberName}` }).click();
		await expect(owner.getByText('No members yet.')).toBeVisible();
		await member.goto('/settings/users');
		await expect(member.getByText(/do not have access|403/i).first()).toBeVisible();
	});
});
