/** `YYYY-MM-DD` for a local date. */
export function isoDay(d: Date): string {
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** The Monday of the week `day` (`YYYY-MM-DD`) is in. */
export function mondayOf(day: string): string {
	const [y, m, d] = day.split('-').map(Number);
	const date = new Date(y, m - 1, d);
	const back = (date.getDay() + 6) % 7;
	date.setDate(date.getDate() - back);
	return isoDay(date);
}

/** The seven days from `monday`. */
export function weekDays(monday: string): string[] {
	const [y, m, d] = monday.split('-').map(Number);
	return Array.from({ length: 7 }, (_, i) => isoDay(new Date(y, m - 1, d + i)));
}

/** `day` moved by `weeks` weeks. */
export function addWeeks(day: string, weeks: number): string {
	const [y, m, d] = day.split('-').map(Number);
	return isoDay(new Date(y, m - 1, d + weeks * 7));
}
