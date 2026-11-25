import type { components } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';

type AssetType = components['schemas']['AssetType'];

export const STATUSES = [
	{ value: 'planned', label: 'Planned' },
	{ value: 'in_stock', label: 'In stock' },
	{ value: 'active', label: 'Active' },
	{ value: 'maintenance', label: 'In maintenance' },
	{ value: 'retired', label: 'Retired' },
	{ value: 'disposed', label: 'Disposed' }
];
export const statusLabel = (s: string) => STATUSES.find((x) => x.value === s)?.label ?? s;
export const statusTone = (s: string) =>
	s === 'active'
		? 'success'
		: s === 'maintenance'
			? 'warning'
			: s === 'retired' || s === 'disposed'
				? 'neutral'
				: 'info';

export const KINDS = [
	{ value: 'runs_on', label: 'runs on', inverse: 'runs' },
	{ value: 'depends_on', label: 'depends on', inverse: 'is depended on by' },
	{ value: 'hosts', label: 'hosts', inverse: 'is hosted by' },
	{ value: 'connects_to', label: 'connects to', inverse: 'is connected from' },
	{ value: 'licensed_by', label: 'is licensed by', inverse: 'licenses' },
	{ value: 'backs_up', label: 'backs up', inverse: 'is backed up by' }
];
/** How a relationship reads from this asset. */
export const relationLabel = (kind: string, direction: string) => {
	const k = KINDS.find((x) => x.value === kind);
	return k ? (direction === 'out' ? k.label : k.inverse) : kind;
};

/** The type fields' values from the form (`attr.<key>`), typed. */
function attributes(form: FormData, type: AssetType | undefined) {
	const out: Record<string, unknown> = {};
	for (const field of type?.fields ?? []) {
		const name = `attr.${field.key}`;
		if (field.fieldType === 'boolean') {
			if (form.get(name) === 'on') out[field.key] = true;
			continue;
		}
		if (field.fieldType === 'multi_select') {
			const values = form.getAll(name).map(String).filter(Boolean);
			if (values.length) out[field.key] = values;
			continue;
		}
		const value = formString(form, name).trim();
		if (!value) continue;
		out[field.key] = field.fieldType === 'number' ? Number(value) : value;
	}
	return out;
}

/** An asset's fields from its form. */
export function assetBody(form: FormData, types: AssetType[]) {
	const typeId = formString(form, 'typeId');
	const optional = (name: string) => formString(form, name).trim() || null;
	return {
		typeId,
		name: formString(form, 'name'),
		status: formString(form, 'status') || 'active',
		serialNumber: optional('serialNumber'),
		hostname: optional('hostname'),
		manufacturer: optional('manufacturer'),
		model: optional('model'),
		siteId: optional('siteId'),
		contactId: optional('contactId'),
		purchasedOn: optional('purchasedOn'),
		warrantyEndsOn: optional('warrantyEndsOn'),
		notes: optional('notes'),
		attributes: attributes(
			form,
			types.find((t) => t.id === typeId)
		)
	};
}
