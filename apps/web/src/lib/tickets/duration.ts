/**
 * Minutes from text like "30m", "4h", "1h 30m", "2d" (a day is 24 hours)
 * or a bare number of minutes. Empty: `null`. Unreadable: `NaN`.
 */
export function parseDuration(text: string): number | null {
	const value = text.trim().toLowerCase();
	if (value === '') return null;
	if (/^\d+$/.test(value)) return Number(value);
	const units: Record<string, number> = { d: 24 * 60, h: 60, m: 1 };
	let total = 0;
	let rest = value;
	const part = /^(\d+(?:\.\d+)?)\s*(d|h|m)\s*/;
	while (rest.length > 0) {
		const match = part.exec(rest);
		if (!match) return Number.NaN;
		total += Number(match[1]) * units[match[2]];
		rest = rest.slice(match[0].length);
	}
	return Math.round(total);
}

/** "1h 30m" for 90; empty for none. */
export function formatDuration(minutes: number | null | undefined): string {
	if (minutes === null || minutes === undefined) return '';
	const hours = Math.floor(minutes / 60);
	const rest = minutes % 60;
	if (hours === 0) return `${rest}m`;
	return rest === 0 ? `${hours}h` : `${hours}h ${rest}m`;
}
