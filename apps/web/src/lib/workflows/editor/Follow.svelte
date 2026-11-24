<script lang="ts">
	import { untrack } from 'svelte';
	import { useSvelteFlow } from '@xyflow/svelte';
	import { NODE_HEIGHT, NODE_WIDTH } from './layout.ts';

	/**
	 * Keeps what you're working on in view: the selected step (with room
	 * below it for its branches' "+"), and anything that gets keyboard focus.
	 */
	let {
		selected,
		width,
		height,
		container
	}: {
		/** The selected step's position on the canvas. */
		selected: { x: number; y: number } | null;
		width: number;
		height: number;
		container: HTMLElement | undefined;
	} = $props();

	const flow = useSvelteFlow();
	const MARGIN = 48;
	/** Room kept visible below a selected step. */
	const BELOW = 180;

	/** Pans the least that shows `at` (and the room below it). */
	function reveal(at: { x: number; y: number }) {
		if (width <= 0 || height <= 0) return;
		const { x, y, zoom } = flow.getViewport();
		const left = at.x * zoom + x;
		const top = at.y * zoom + y;
		const right = left + NODE_WIDTH * zoom;
		const bottom = top + (NODE_HEIGHT + BELOW) * zoom;
		let dx = 0;
		let dy = 0;
		if (left < MARGIN) dx = MARGIN - left;
		else if (right > width - MARGIN) dx = width - MARGIN - right;
		if (top < MARGIN) dy = MARGIN - top;
		else if (bottom > height - MARGIN) dy = Math.max(height - MARGIN - bottom, MARGIN - top);
		if (dx !== 0 || dy !== 0) void flow.setViewport({ x: x + dx, y: y + dy, zoom });
	}

	// Only when the selection changes: not when the view moves (which this
	// itself does).
	$effect(() => {
		const at = selected;
		if (at) untrack(() => reveal(at));
	});

	// Tabbing to something off-screen pans it into view.
	$effect(() => {
		if (!container) return;
		const element = container;
		const onfocus = (event: FocusEvent) => {
			const target = event.target as HTMLElement | null;
			if (!target || target === element) return;
			// The browser scrolls clipped boxes to show what got focus,
			// which would shift the whole canvas: undo that, then pan.
			for (let box = target.parentElement; box; box = box.parentElement) {
				box.scrollTop = 0;
				box.scrollLeft = 0;
				if (box === element) break;
			}
			const box = element.getBoundingClientRect();
			const rect = target.getBoundingClientRect();
			let dx = 0;
			let dy = 0;
			if (rect.left < box.left + MARGIN) dx = box.left + MARGIN - rect.left;
			else if (rect.right > box.right - MARGIN) dx = box.right - MARGIN - rect.right;
			if (rect.top < box.top + MARGIN) dy = box.top + MARGIN - rect.top;
			else if (rect.bottom > box.bottom - MARGIN) dy = box.bottom - MARGIN - rect.bottom;
			if (dx === 0 && dy === 0) return;
			const { x, y, zoom } = flow.getViewport();
			void flow.setViewport({ x: x + dx, y: y + dy, zoom });
		};
		element.addEventListener('focusin', onfocus);
		return () => element.removeEventListener('focusin', onfocus);
	});
</script>
