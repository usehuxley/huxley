import { expect, request, test, type Page } from '@playwright/test';

// Full-stack: needs the API with a bootstrapped owner and HUXLEY_API_URL,
// and the workforce module on (the e2e job's service-desk preset).
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

/** A technician (through the API), with `level` in `skill`; returns their id. */
async function technician(name: string, skillName: string, level: number): Promise<string> {
	const api = await request.newContext({
		baseURL: apiUrl,
		extraHTTPHeaders: { 'x-requested-with': 'huxley' }
	});
	await api.post('/api/v1/auth/login', { data: { email: ownerEmail, password: ownerPassword } });
	const email = `${name.toLowerCase().replace(/\W+/g, '.')}@example.com`;
	const created = await api.post('/api/v1/users', {
		data: { email, displayName: name, password: 'a long enough password 1' }
	});
	expect(created.status(), await created.text()).toBe(201);
	const id = ((await created.json()) as { user: { id: string } }).user.id;
	const skills = (await (await api.get('/api/v1/workforce/skills')).json()) as {
		id: string;
		name: string;
	}[];
	const skill = skills.find((s) => s.name === skillName)!;
	const rated = await api.put(`/api/v1/workforce/people/${id}/skills/${skill.id}`, {
		data: { level }
	});
	expect(rated.status()).toBe(200);
	await api.dispose();
	return id;
}

test.describe('workforce', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the workforce module is off in this run');

	test('skills and certifications are kept, verified and required', async ({ page }) => {
		const stamp = Date.now().toString(36);
		const skill = `Intune ${stamp}`;
		const cert = `Endpoint Administrator ${stamp}`;
		await signIn(page);

		// The catalogs.
		await page.goto('/settings/skills');
		await page.getByRole('button', { name: 'New skill' }).click();
		let dialog = page.getByRole('dialog');
		await dialog.getByLabel('Name').fill(skill);
		await dialog.getByLabel('Category').fill('Microsoft 365');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText(skill)).toBeVisible();
		await page.getByRole('button', { name: 'New certification' }).click();
		dialog = page.getByRole('dialog');
		await dialog.getByLabel('Vendor').fill('Microsoft');
		await dialog.getByLabel('Code').fill('MD-102');
		await dialog.getByLabel('Name', { exact: true }).fill(cert);
		await dialog.getByLabel('Valid for (months)').fill('12');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText(`Microsoft ${cert}`)).toBeVisible();

		// My own skill (self-assessed) and certification.
		await page.getByRole('link', { name: 'People', exact: true }).click();
		await page
			.getByRole('table', { name: 'People' })
			.getByRole('link', { name: 'E2E User' })
			.click();
		await page.getByLabel('Add a skill').selectOption({ label: `${skill} (Microsoft 365)` });
		await page.getByLabel('Level', { exact: true }).selectOption('3');
		await page.getByRole('button', { name: 'Add', exact: true }).first().click();
		await expect(page.getByText('Skill added.')).toBeVisible();
		await expect(page.getByText('Microsoft 365 · Self-assessed')).toBeVisible();
		await page
			.locator('section')
			.filter({ has: page.getByRole('heading', { name: 'Certifications' }) })
			.getByRole('button', { name: 'Add' })
			.click();
		dialog = page.getByRole('dialog');
		await dialog.getByLabel('Certification').selectOption({ label: `Microsoft ${cert} (MD-102)` });
		await dialog.getByLabel('Earned').fill('2026-06-01');
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('Earned 2026-06-01 · expires 2027-06-01')).toBeVisible();
		await expect(page.getByText('Valid', { exact: true })).toBeVisible();

		// A colleague's level, verified by me; it shows in the matrix.
		const name = `Tess ${stamp}`;
		await technician(name, skill, 2);
		await page.goto('/people');
		await page.getByRole('link', { name }).click();
		await page.getByRole('button', { name: 'Verify' }).click();
		await expect(page.getByText(/Verified by E2E User on/)).toBeVisible();
		await page.goto('/people?view=matrix');
		const row = page.getByRole('row', { name: new RegExp(name) });
		await expect(row.getByRole('cell', { name: /2/ })).toBeVisible();

		// Booked off today by me (a manager): approved at once, and away.
		await page.goto('/people');
		await page.getByRole('table', { name: 'People' }).getByRole('link', { name }).click();
		await page.getByRole('button', { name: 'Book' }).click();
		dialog = page.getByRole('dialog');
		const today = new Date().toISOString().slice(0, 10);
		await dialog.getByLabel('From').fill(today);
		await dialog.getByLabel('To (inclusive)').fill(today);
		await dialog.getByLabel('Kind').selectOption('training');
		await dialog.getByRole('button', { name: 'Book' }).click();
		await expect(page.getByText('Booked.')).toBeVisible();
		await expect(page.getByText('Approved', { exact: true })).toBeVisible();
		await page.goto('/people');
		await expect(
			page.getByRole('row', { name: new RegExp(name) }).getByText('Away today')
		).toBeVisible();

		// My own hours, and time off that waits for someone else's approval.
		await page
			.getByRole('table', { name: 'People' })
			.getByRole('link', { name: 'E2E User' })
			.click();
		await page.getByLabel('Time zone').fill('Europe/London');
		await page.getByRole('button', { name: 'Save hours' }).click();
		await expect(page.getByText('Working hours saved.')).toBeVisible();
		await expect(page.getByText('In Europe/London.')).toBeVisible();
		await page.getByRole('button', { name: 'Request' }).click();
		dialog = page.getByRole('dialog');
		await dialog.getByLabel('From').fill('2099-07-01');
		await dialog.getByLabel('To (inclusive)').fill('2099-07-05');
		await dialog.getByRole('button', { name: 'Request' }).click();
		await expect(page.getByText('Waiting for approval')).toBeVisible();
		await expect(page.getByRole('button', { name: 'Approve' })).toBeHidden();
		await page.goto('/people');
		await expect(page.getByText('Time off waiting for approval')).toBeVisible();

		// A queue that requires them.
		await page.goto('/settings/queues');
		await page.getByLabel('Name', { exact: true }).fill(`Endpoints ${stamp}`);
		await page.getByRole('button', { name: 'Create queue' }).click();
		const queue = page.getByRole('listitem').filter({ hasText: `Endpoints ${stamp}` });
		await queue.getByRole('button', { name: 'Requirements' }).click();
		dialog = page.getByRole('dialog');
		await dialog.getByLabel('Skill 1', { exact: true }).selectOption({ label: skill });
		await dialog.getByLabel('Skill 1 level').selectOption('3');
		await dialog.getByLabel(`Microsoft ${cert}`).check();
		await dialog.getByRole('button', { name: 'Save' }).click();
		await expect(queue.getByText(`Requires Microsoft ${cert}, ${skill} (level 3)`)).toBeVisible();
	});
});
