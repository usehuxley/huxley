export type Holiday = { date: string; name: string };

/**
 * Holidays from a text box: one a line, a date (`YYYY-MM-DD`) then an
 * optional name, e.g. `2026-12-25 Christmas Day`. Blank lines are skipped;
 * the API checks the dates.
 */
export function parseHolidays(text: string): Holiday[] {
	return text
		.split('\n')
		.map((line) => line.trim())
		.filter(Boolean)
		.map((line) => {
			const [date = '', ...rest] = line.split(/\s+/);
			return { date, name: rest.join(' ') };
		});
}

/** Holidays for the text box. */
export function formatHolidays(list: Holiday[]): string {
	return list.map((h) => (h.name ? `${h.date} ${h.name}` : h.date)).join('\n');
}
