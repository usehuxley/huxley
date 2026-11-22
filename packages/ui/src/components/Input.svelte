<script lang="ts">
	import type { HTMLInputAttributes } from 'svelte/elements';

	type Props = Omit<HTMLInputAttributes, 'value'> & {
		value?: string | number | null;
		/** Marks the field invalid (sets aria-invalid and error styling). */
		invalid?: boolean;
	};

	let { value = $bindable(), invalid = false, class: className = '', ...rest }: Props = $props();
</script>

<input
	bind:value
	aria-invalid={invalid || undefined}
	class={[
		'h-9 w-full rounded-control border bg-surface px-3 text-sm text-fg',
		'placeholder:text-fg-subtle disabled:opacity-50',
		invalid ? 'border-danger' : 'border-border-strong',
		className
	].join(' ')}
	{...rest}
/>
