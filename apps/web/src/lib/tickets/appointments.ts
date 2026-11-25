/** A local date and time (`YYYY-MM-DD`, `HH:MM`) as an RFC 3339 instant. */
export function localInstant(day: string, time: string): string {
	const [y, m, d] = day.split('-').map(Number);
	const [h, min] = time.split(':').map(Number);
	return new Date(y, m - 1, d, h, min).toISOString();
}

/** The instant `minutes` after `iso`. */
export function plusMinutes(iso: string, minutes: number): string {
	return new Date(new Date(iso).getTime() + minutes * 60_000).toISOString();
}

/** "Tue 3 Mar, 09:00–11:00" in this browser's time. */
export function span(startsAt: string, endsAt: string): string {
	const s = new Date(startsAt);
	const e = new Date(endsAt);
	const day = s.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short' });
	const clock = (d: Date) =>
		d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
	return `${day}, ${clock(s)}–${clock(e)}`;
}
