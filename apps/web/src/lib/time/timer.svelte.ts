/**
 * Timers for logging time, one per key (a ticket's id, or `general`), kept
 * in this browser so they survive reloads. Browser storage can be missing
 * (private windows): then a timer lasts until the page closes.
 */
const KEY = 'huxley.timers';

function load(): Record<string, number> {
	try {
		const raw = localStorage.getItem(KEY);
		return raw ? (JSON.parse(raw) as Record<string, number>) : {};
	} catch {
		return {};
	}
}

let tick: ReturnType<typeof setInterval> | undefined;

class Timers {
	started: Record<string, number> = $state({});
	now = $state(Date.now());

	/** Reads what this browser kept; call once the page is in a browser. */
	init() {
		this.started = load();
		tick ??= setInterval(() => (this.now = Date.now()), 1000);
	}

	#save() {
		try {
			localStorage.setItem(KEY, JSON.stringify(this.started));
		} catch {
			// Kept for this page only.
		}
	}

	running(key: string): boolean {
		return key in this.started;
	}

	/** Seconds so far. */
	elapsed(key: string): number {
		const at = this.started[key];
		return at === undefined ? 0 : Math.max(0, Math.floor((this.now - at) / 1000));
	}

	start(key: string) {
		this.started = { ...this.started, [key]: Date.now() };
		this.#save();
	}

	/** Stops it: the whole minutes, at least 1. */
	stop(key: string): number {
		const seconds = this.elapsed(key);
		this.started = Object.fromEntries(Object.entries(this.started).filter(([k]) => k !== key));
		this.#save();
		return Math.max(1, Math.ceil(seconds / 60));
	}
}

export const timers = new Timers();

/** `1:05:09` or `5:09`. */
export function clock(seconds: number): string {
	const h = Math.floor(seconds / 3600);
	const m = Math.floor((seconds % 3600) / 60);
	const s = seconds % 60;
	const two = (n: number) => String(n).padStart(2, '0');
	return h > 0 ? `${h}:${two(m)}:${two(s)}` : `${m}:${two(s)}`;
}
