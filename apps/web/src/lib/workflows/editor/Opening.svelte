<script lang="ts">
	import { useSvelteFlow } from '@xyflow/svelte';

	/**
	 * Opens the canvas on the top of the workflow, centred, zoomed to fit
	 * its width (between 60% and 100%): fitting a long workflow whole would
	 * make it too small to read. Runs once.
	 */
	let { width, left, right }: { width: number; left: number; right: number } = $props();

	const flow = useSvelteFlow();
	let done = false;
	$effect(() => {
		if (done || width <= 0) return;
		done = true;
		const graph = Math.max(1, right - left);
		const zoom = Math.min(1, Math.max(0.6, (width - 48) / graph));
		void flow.setViewport({ x: (width - graph * zoom) / 2 - left * zoom, y: 24, zoom });
	});
</script>
