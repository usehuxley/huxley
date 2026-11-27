import type { Page } from '@playwright/test';

/** Spaces on the rail, and the menu items in each. */
const SPACE_OF: Record<string, string> = {
	'My work': 'Home',
	'My time': 'Home',
	Approvals: 'Home',
	Tickets: 'Desk',
	Knowledge: 'Clients',
	Workflows: 'Engine',
	Forms: 'Engine',
	Scripts: 'Engine',
	Integrations: 'Engine',
	Datasets: 'Engine',
	Dashboards: 'Business',
	Catalog: 'Business',
	Inventory: 'Business',
	Purchasing: 'Business',
	Pipeline: 'Business',
	Leads: 'Business',
	Quotes: 'Business',
	Orders: 'Business',
	Activities: 'Business',
	People: 'Business',
	Timesheets: 'Business'
};

/** Opens a page the way a person would: its space on the rail, then the space's menu. */
export async function openFromMenu(page: Page, item: string) {
	const space = SPACE_OF[item];
	if (!space) throw new Error(`No space for ${item}`);
	await page
		.getByRole('navigation', { name: 'Spaces' })
		.getByRole('link', { name: space, exact: true })
		.click();
	const link = page
		.getByRole('complementary', { name: `${space} menu` })
		.getByRole('link', { name: item, exact: true });
	const href = await link.getAttribute('href');
	await link.click();
	// Wait for the page itself: the one before may have similar fields.
	await page.waitForURL((url) => url.pathname === href);
	await page.waitForLoadState();
}
