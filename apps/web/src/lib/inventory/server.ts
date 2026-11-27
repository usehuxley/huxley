import { fail, type RequestEvent } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { serialList } from '#lib/inventory/labels.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';

type RefType = 'ticket' | 'project';
type User = { activeModules: string[]; grantedPermissions: string[] };

/** Parts on a ticket or project, and (for those who take parts) what can be taken. */
export async function partsFor(event: RequestEvent, refType: RefType, refId: string, user: User) {
	if (
		!user.activeModules.includes('inventory') ||
		!user.grantedPermissions.includes('inventory:read')
	) {
		return null;
	}
	const client = api(event);
	const canWrite = user.grantedPermissions.includes('inventory:write');
	const [allocations, items, locations] = await Promise.all([
		client.GET('/api/v1/inventory/allocations', { params: { query: { refType, refId } } }),
		canWrite ? client.GET('/api/v1/inventory/items') : null,
		canWrite ? client.GET('/api/v1/inventory/locations') : null
	]);
	return {
		allocations: allocations.data ?? [],
		items: (items?.data ?? [])
			.filter((i) => Number(i.available) > 0)
			.map((i) => ({
				id: i.id,
				orgId: i.orgId,
				label: `${i.name} · ${i.sku} (${Number(i.available)} available)`,
				tracking: i.tracking
			})),
		locations: (locations?.data ?? []).map((l) => ({
			value: l.id,
			orgId: l.orgId,
			label: l.parentName ? `${l.parentName} › ${l.name}` : l.name
		})),
		canWrite
	};
}

/** Form action: takes a part for the ticket or project. */
export async function addPart(event: RequestEvent, refType: RefType, refId: string) {
	const form = await event.request.formData();
	const quantity = formString(form, 'quantity').trim();
	const { data, error, response } = await api(event).POST('/api/v1/inventory/allocations', {
		body: {
			itemId: formString(form, 'itemId'),
			locationId: formString(form, 'locationId'),
			quantity: quantity || null,
			lot: formString(form, 'lot') || null,
			serials: serialList(formString(form, 'serials')),
			refType,
			refId,
			note: formString(form, 'note') || null,
			consume: form.get('consume') === 'on'
		}
	});
	if (!data) {
		return fail(response.status, { message: problemMessage(error, 'Could not take the part.') });
	}
	return { message: data.status === 'consumed' ? 'Part used.' : 'Part set aside.' };
}

/** Form action: a part set aside was used, or isn't needed. */
export async function closePart(event: RequestEvent) {
	const form = await event.request.formData();
	const id = formString(form, 'allocationId');
	const params = { params: { path: { id } } };
	const used = formString(form, 'to') === 'consume';
	const { data, error, response } = used
		? await api(event).POST('/api/v1/inventory/allocations/{id}/consume', params)
		: await api(event).POST('/api/v1/inventory/allocations/{id}/release', params);
	if (!data) {
		return fail(response.status, { message: problemMessage(error, 'Could not change it.') });
	}
	return { message: used ? 'Part used.' : 'Part put back.' };
}
