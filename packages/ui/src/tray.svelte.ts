// The work tray: what someone has open (tickets, clients, runs, ...) as tabs
// across the top of the app, so switching between them never loses the place.
// Kept per browser in localStorage; pages add themselves with `tray.open()`.

export type TrayKind = 'ticket' | 'client' | 'run' | 'workflow' | 'project' | 'other';

export type TrayItem = {
	/** Stable identity, e.g. `ticket:4812`. */
	key: string;
	kind: TrayKind;
	href: string;
	title: string;
	/** Second line, e.g. the client. */
	subtitle?: string;
	/** A countdown to show as a ring, e.g. a ticket's resolution SLA. */
	clock?: { from: string; due: string; paused?: boolean } | null;
	/** For runs: whether it is still going. */
	state?: 'running' | 'done' | 'failed' | null;
	/** Shown as a coloured initial for clients. */
	color?: string | null;
};

import { untrack } from 'svelte';

const STORAGE_KEY = 'hx-tray';
const MAX_ITEMS = 12;

function load(): TrayItem[] {
	try {
		const raw = localStorage.getItem(STORAGE_KEY);
		const parsed: unknown = raw ? JSON.parse(raw) : [];
		return Array.isArray(parsed)
			? parsed.filter(
					(i): i is TrayItem => typeof i?.key === 'string' && typeof i?.href === 'string'
				)
			: [];
	} catch {
		return [];
	}
}

class Tray {
	items = $state<TrayItem[]>([]);
	#loaded = false;

	/** Reads the saved tray. Call once in the browser (the shell does). */
	restore() {
		if (this.#loaded) return;
		this.#loaded = true;
		this.items = load();
	}

	#save() {
		try {
			localStorage.setItem(STORAGE_KEY, JSON.stringify(this.items));
		} catch {
			/* not persisted */
		}
	}

	/** Adds an item, or refreshes it in place if it is already open. */
	open(item: TrayItem) {
		// Pages call this from an effect: don't let the effect depend on the tray.
		untrack(() => this.#open(item));
	}

	#open(item: TrayItem) {
		this.restore();
		const at = this.items.findIndex((i) => i.key === item.key);
		if (at >= 0) {
			this.items[at] = { ...this.items[at], ...item };
		} else {
			this.items = [...this.items, item].slice(-MAX_ITEMS);
		}
		this.#save();
	}

	/** Updates an open item without opening it. */
	update(key: string, patch: Partial<TrayItem>) {
		untrack(() => this.#update(key, patch));
	}

	#update(key: string, patch: Partial<TrayItem>) {
		const at = this.items.findIndex((i) => i.key === key);
		if (at < 0) return;
		this.items[at] = { ...this.items[at], ...patch };
		this.#save();
	}

	close(key: string) {
		this.items = this.items.filter((i) => i.key !== key);
		this.#save();
	}

	clear() {
		this.items = [];
		this.#save();
	}
}

/** The one tray for this browser tab. */
export const tray = new Tray();

/** How much of a clock has run, 0 to 1 (over 1 when overdue). */
export function clockUsed(clock: { from: string; due: string }, now: number): number {
	const from = new Date(clock.from).getTime();
	const due = new Date(clock.due).getTime();
	if (!(due > from)) return now >= due ? 1 : 0;
	return Math.max(0, (now - from) / (due - from));
}

/** The tone of a clock: on track, past half, or in its last quarter or breached. */
export function clockTone(used: number): 'ok' | 'warn' | 'danger' {
	return used >= 0.75 ? 'danger' : used >= 0.5 ? 'warn' : 'ok';
}
