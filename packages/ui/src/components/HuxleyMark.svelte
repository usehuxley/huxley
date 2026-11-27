<script lang="ts">
	import mark from '../assets/huxley-mark.png';

	/** The Huxley butler, for things the engine did. Decorative unless labelled. */
	let { size = 28, label = '' }: { size?: number; label?: string } = $props();
</script>

<span
	class="grid flex-none place-items-center overflow-hidden rounded-full border border-accent/30 bg-accent-soft"
	style="width:{size}px;height:{size}px"
>
	<img
		src={mark}
		alt={label}
		aria-hidden={label ? undefined : 'true'}
		style="width:{size - 4}px;height:{size - 4}px"
		class="object-contain"
	/>
</span>
