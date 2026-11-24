<script lang="ts">
	import { useSvelteFlow } from '@xyflow/svelte';
	import type { Direction } from './layout.ts';

	/**
	 * Opens the canvas on the start of the workflow, as much of it as fits
	 * at 60% to 100% zoom: top to bottom from the top, centred across; left
	 * to right from the left, centred down. Fitting a long workflow
	 * whole would make it too small to read. Runs again when the direction
	 * changes.
	 */
	let {
		width,
		height,
		bounds,
		direction
	}: {
		width: number;
		height: number;
		bounds: { left: number; top: number; right: number; bottom: number };
		direction: Direction;
	} = $props();

	const flow = useSvelteFlow();
	let shown: Direction | null = null;
	$effect(() => {
		if (shown === direction || width <= 0 || height <= 0) return;
		shown = direction;
		// As much as fits in both directions, but never below 60%.
		const graphWidth = Math.max(1, bounds.right - bounds.left);
		const graphHeight = Math.max(1, bounds.bottom - bounds.top);
		const zoom = Math.min(
			1,
			Math.max(0.6, Math.min((width - 48) / graphWidth, (height - 48) / graphHeight))
		);
		const across = direction === 'down' ? graphWidth : graphHeight;
		void flow.setViewport(
			direction === 'down'
				? { x: (width - across * zoom) / 2 - bounds.left * zoom, y: 24 - bounds.top * zoom, zoom }
				: { x: 24 - bounds.left * zoom, y: (height - across * zoom) / 2 - bounds.top * zoom, zoom }
		);
	});
</script>
