/**
 * Toast notifications. Call `toast.push(...)` anywhere; render one
 * `<Toaster />` near the root of the app.
 */

export type ToastTone = 'neutral' | 'success' | 'warning' | 'danger';

export type Toast = {
	id: number;
	title: string;
	description?: string;
	tone: ToastTone;
};

const DEFAULT_DURATION_MS = 5000;

class ToastStore {
	items = $state<Toast[]>([]);
	#nextId = 1;

	push(input: { title: string; description?: string; tone?: ToastTone; durationMs?: number }) {
		const id = this.#nextId++;
		this.items.push({
			id,
			title: input.title,
			description: input.description,
			tone: input.tone ?? 'neutral'
		});
		const duration = input.durationMs ?? DEFAULT_DURATION_MS;
		if (duration > 0) setTimeout(() => this.dismiss(id), duration);
		return id;
	}

	dismiss(id: number) {
		this.items = this.items.filter((t) => t.id !== id);
	}
}

export const toast = new ToastStore();
