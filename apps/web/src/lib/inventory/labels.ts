import type { BadgeTone } from '@usehuxley/ui';

export const LOCATION_KINDS = [
	{ value: 'warehouse', label: 'Warehouse' },
	{ value: 'bin', label: 'Bin (in a warehouse)' },
	{ value: 'vehicle', label: 'Vehicle' },
	{ value: 'site', label: 'Client site' }
] as const;

export const TRACKING = [
	{ value: 'none', label: 'By quantity' },
	{ value: 'serial', label: 'By serial number' },
	{ value: 'lot', label: 'By lot' }
] as const;

/** The movements people make by hand (allocations come from tickets and projects). */
export const MOVES = [
	{ value: 'receive', label: 'Receive' },
	{ value: 'transfer', label: 'Transfer' },
	{ value: 'adjust', label: 'Adjust' },
	{ value: 'return', label: 'Return from a job' },
	{ value: 'rma', label: 'Send back (RMA)' }
] as const;

export const MOVEMENT_KINDS: Record<string, string> = {
	receive: 'Received',
	transfer: 'Transferred',
	adjust: 'Adjusted',
	allocate: 'Allocated',
	release: 'Released',
	consume: 'Used',
	return: 'Returned',
	rma: 'Sent back (RMA)'
};

export const UNIT_STATUSES: Record<string, { label: string; tone: BadgeTone }> = {
	in_stock: { label: 'In stock', tone: 'success' },
	allocated: { label: 'Allocated', tone: 'info' },
	consumed: { label: 'Used', tone: 'neutral' },
	rma: { label: 'RMA', tone: 'warning' },
	returned: { label: 'Written off', tone: 'neutral' }
};

export const label = (list: readonly { value: string; label: string }[], value: string) =>
	list.find((x) => x.value === value)?.label ?? value;

/** A quantity, as given (decimal strings), grouped. */
export const qty = (n: string | number) =>
	Number(n).toLocaleString(undefined, { maximumFractionDigits: 4 });

/** Serial numbers typed one per line, or separated by commas. */
export const serialList = (text: string) =>
	text
		.split(/[\n,]/)
		.map((s) => s.trim())
		.filter(Boolean);
