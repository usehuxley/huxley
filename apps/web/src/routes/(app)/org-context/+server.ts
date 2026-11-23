import { json, type RequestHandler } from '@sveltejs/kit';
import { ORG_COOKIE, type OrgOption } from '#lib/orgContext.ts';
import { api } from '#lib/server/api.ts';

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/** The organizations the switcher offers: everything the user can see. */
export const GET: RequestHandler = async (event) => {
	const { data } = await api(event).GET('/api/v1/orgs');
	const orgs = data ?? [];
	const names = new Map(orgs.map((o) => [o.id, o.name]));
	const options: OrgOption[] = orgs.map((o) => ({
		id: o.id,
		name: o.name,
		kind: o.kind,
		parentName: o.parentId ? (names.get(o.parentId) ?? null) : null
	}));
	return json(options, { headers: { 'cache-control': 'private, no-store' } });
};

/** Switches organization (`{ orgId }`), or back to all (`{ orgId: null }`). */
export const POST: RequestHandler = async (event) => {
	const body: unknown = await event.request.json().catch(() => null);
	const orgId =
		body && typeof body === 'object' && 'orgId' in body && typeof body.orgId === 'string'
			? body.orgId
			: null;
	if (!orgId) {
		event.cookies.delete(ORG_COOKIE, { path: '/' });
		return json({ org: null });
	}
	if (!UUID.test(orgId)) return json({ message: 'Unknown organization.' }, { status: 400 });
	const { data } = await api(event).GET('/api/v1/orgs/{id}', { params: { path: { id: orgId } } });
	if (!data) return json({ message: 'Unknown organization.' }, { status: 404 });
	event.cookies.set(ORG_COOKIE, data.id, {
		path: '/',
		httpOnly: true,
		sameSite: 'lax',
		maxAge: 60 * 60 * 24 * 365
	});
	return json({ org: { id: data.id, name: data.name } });
};
