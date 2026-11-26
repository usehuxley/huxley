import type { RequestHandler } from '@sveltejs/kit';
import { apiHeaders, apiUrl } from '#lib/server/api.ts';

const DAY = /^\d{4}-\d{2}-\d{2}$/;

/** The payroll export (approved time and time off), through the app. */
export const GET: RequestHandler = async (event) => {
	const from = event.url.searchParams.get('from') ?? '';
	const to = event.url.searchParams.get('to') ?? '';
	if (!DAY.test(from) || !DAY.test(to))
		return new Response('Give from and to dates.', { status: 400 });
	const upstream = await fetch(`${apiUrl}/api/v1/timesheets/payroll.csv?from=${from}&to=${to}`, {
		headers: apiHeaders(event),
		signal: event.request.signal
	}).catch(() => null);
	if (!upstream?.ok || !upstream.body) {
		const text = upstream ? await upstream.text().catch(() => '') : '';
		return new Response(text || 'Could not export.', { status: upstream?.status ?? 502 });
	}
	const headers = new Headers();
	for (const name of ['content-type', 'content-disposition']) {
		const value = upstream.headers.get(name);
		if (value) headers.set(name, value);
	}
	return new Response(upstream.body, { headers });
};
