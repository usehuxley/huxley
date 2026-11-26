import type { BadgeTone } from '@usehuxley/ui';

/** A device's warranty as a short label: ended, ending soon, or until when. */
export function warrantyText(
	endsOn: string | null | undefined,
	today = new Date()
): { text: string; tone: BadgeTone } | null {
	if (!endsOn) return null;
	const end = new Date(`${endsOn}T00:00:00`);
	const days = Math.floor((end.getTime() - today.getTime()) / 86_400_000);
	const date = end.toLocaleDateString(undefined, {
		year: 'numeric',
		month: 'short',
		day: 'numeric'
	});
	if (days < 0) return { text: `Warranty ended ${date}`, tone: 'danger' };
	if (days <= 60) return { text: `Warranty ends ${date}`, tone: 'warning' };
	return { text: `Warranty until ${date}`, tone: 'neutral' };
}
