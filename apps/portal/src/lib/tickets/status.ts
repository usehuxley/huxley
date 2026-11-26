import type { BadgeTone } from '@usehuxley/ui';

/** How a status category looks in the portal. */
export function statusTone(category: string): BadgeTone {
	switch (category) {
		case 'new':
		case 'open':
			return 'info';
		case 'pending':
			return 'warning';
		case 'resolved':
			return 'success';
		default:
			return 'neutral';
	}
}

/** A date and time, short. */
export function when(iso: string): string {
	return new Date(iso).toLocaleString(undefined, {
		day: 'numeric',
		month: 'short',
		hour: '2-digit',
		minute: '2-digit'
	});
}
