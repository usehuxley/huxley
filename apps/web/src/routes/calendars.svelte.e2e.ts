import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, request, test, type APIRequestContext } from '@playwright/test';

// Full-stack (ADR-061): a person's calendar feed is synced by the Calendar
// sync pack, and their busy time shows on the dispatch board. The feed is
// served here, on 127.0.0.1 (the e2e API lets the engine reach it).
const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;
const apiUrl = process.env.HUXLEY_API_URL;
const preset = process.env.E2E_PRESET ?? 'automation';

const pad = (n: number) => String(n).padStart(2, '0');
const utc = (d: Date) =>
	`${d.getUTCFullYear()}${pad(d.getUTCMonth() + 1)}${pad(d.getUTCDate())}T${pad(d.getUTCHours())}${pad(d.getUTCMinutes())}00Z`;
const localDay = (d: Date) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;

async function json<T>(
	r: Promise<import('@playwright/test').APIResponse>,
	status = 200
): Promise<T> {
	const res = await r;
	expect(res.status(), await res.text()).toBe(status);
	return (await res.json()) as T;
}

/** Serves an iCalendar feed with one meeting: tomorrow, 14:00–15:00 local. */
async function feed(stamp: string): Promise<{ url: string; server: Server; start: Date }> {
	const start = new Date();
	start.setDate(start.getDate() + 1);
	start.setHours(14, 0, 0, 0);
	const end = new Date(start.getTime() + 3_600_000);
	const body = [
		'BEGIN:VCALENDAR',
		'VERSION:2.0',
		'BEGIN:VEVENT',
		`UID:${stamp}`,
		`DTSTART:${utc(start)}`,
		`DTEND:${utc(end)}`,
		`SUMMARY:Supplier call ${stamp}`,
		'END:VEVENT',
		'END:VCALENDAR',
		''
	].join('\r\n');
	const server = createServer((_req, res) => {
		res.writeHead(200, { 'content-type': 'text/calendar' });
		res.end(body);
	});
	await new Promise<void>((done) => server.listen(0, '127.0.0.1', done));
	const { port } = server.address() as AddressInfo;
	return { url: `http://127.0.0.1:${port}/feed.ics`, server, start };
}

/** Installs and publishes the built-in Calendar sync pack: its workflow's id. */
async function calendarPack(api: APIRequestContext, root: string): Promise<string> {
	const review = await json<{ uploadId: string; version: string }>(
		api.post(`/api/v1/packs/builtin/huxley/calendar-sync?orgId=${root}`),
		201
	);
	const installed = await json<{ items: { workflows: Record<string, string> } }>(
		api.post(`/api/v1/packs/uploads/${review.uploadId}/install`, {
			data: { orgId: root, version: review.version }
		}),
		201
	);
	const id = Object.values(installed.items.workflows)[0];
	await json(api.post(`/api/v1/workflows/${id}/publish`, { data: {} }));
	return id;
}

test.describe('calendars', () => {
	test.skip(
		!ownerEmail || !ownerPassword || !apiUrl,
		'set E2E_EMAIL, E2E_PASSWORD, and HUXLEY_API_URL'
	);
	test.skip(preset === 'automation', 'the service desk and workforce are off in this run');

	test('busy time from a calendar feed shows on the dispatch board', async ({ page }) => {
		const stamp = `${Date.now().toString(36)}${Math.random().toString(36).slice(2, 6)}`;
		const { url, server, start } = await feed(stamp);
		const api = await request.newContext({
			baseURL: apiUrl,
			extraHTTPHeaders: { 'x-requested-with': 'huxley' }
		});
		try {
			await api.post('/api/v1/auth/login', {
				data: { email: ownerEmail, password: ownerPassword }
			});
			const me = (await json<{ id: string }>(api.get('/api/v1/auth/me'))).id;
			const orgs = await json<{ id: string; parentId: string | null }[]>(api.get('/api/v1/orgs'));
			const root = orgs.find((o) => o.parentId === null)!.id;
			await json(
				api.put(`/api/v1/workforce/calendars/people/${me}`, {
					data: { source: 'ics', icsUrl: url }
				})
			);
			// Booked on a ticket the day after tomorrow, so I'm on the board.
			const ticket = await json<{ id: string }>(
				api.post('/api/v1/tickets', { data: { orgId: root, title: `On site ${stamp}` } }),
				201
			);
			const later = new Date(start.getTime() + 86_400_000);
			await json(
				api.post(`/api/v1/tickets/${ticket.id}/appointments`, {
					data: {
						userId: me,
						startsAt: later.toISOString(),
						endsAt: new Date(later.getTime() + 3_600_000).toISOString()
					}
				}),
				201
			);
			const workflow = await calendarPack(api, root);
			await json(api.post(`/api/v1/workflows/${workflow}/runs`, { data: { inputs: {} } }), 201);
			await expect
				.poll(
					async () =>
						(await json<{ status: string }[]>(api.get(`/api/v1/runs?workflowId=${workflow}`)))[0]
							?.status,
					{ timeout: 20_000 }
				)
				.toBe('succeeded');
		} finally {
			await api.dispose();
			server.close();
		}

		await page.goto('/login');
		await page.getByLabel('Email').fill(ownerEmail!);
		await page.getByLabel('Password').fill(ownerPassword!);
		await page.getByRole('button', { name: 'Sign in', exact: true }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText(
			/^Good (morning|afternoon|evening), /
		);

		// The week with tomorrow in it: the meeting shows as busy (its subject
		// isn't kept by default).
		const monday = new Date(start);
		monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
		await page.goto(`/dispatch?week=${localDay(monday)}`);
		const row = page
			.getByRole('row')
			.filter({ has: page.getByRole('rowheader', { name: 'E2E User' }) });
		await expect(row).toContainText('Busy');
		await expect(row).not.toContainText(`Supplier call ${stamp}`);

		await page.goto('/settings/calendars');
		const mine = page
			.getByRole('list', { name: "People's calendars" })
			.getByRole('listitem')
			.filter({ hasText: ownerEmail! });
		await expect(mine).toContainText('Synced');
	});
});
