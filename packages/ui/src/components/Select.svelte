<script lang="ts" module>
	export type SelectOption = { value: string; label: string; disabled?: boolean };
</script>

<script lang="ts">
	import type { HTMLSelectAttributes } from 'svelte/elements';

	type Props = Omit<HTMLSelectAttributes, 'value'> & {
		value?: string;
		options: SelectOption[];
		/** Shown as an empty first option when set. */
		placeholder?: string;
		invalid?: boolean;
	};

	let {
		value = $bindable(),
		options,
		placeholder,
		invalid = false,
		class: className = '',
		...rest
	}: Props = $props();
</script>

<!-- A styled native select: accessible and mobile-friendly by default. -->
<select
	bind:value
	aria-invalid={invalid || undefined}
	class={[
		'h-9 w-full rounded-control border bg-surface px-3 text-sm text-fg disabled:opacity-50',
		invalid ? 'border-danger' : 'border-border-strong',
		className
	].join(' ')}
	{...rest}
>
	{#if placeholder}
		<option value="" disabled>{placeholder}</option>
	{/if}
	{#each options as option (option.value)}
		<option value={option.value} disabled={option.disabled}>{option.label}</option>
	{/each}
</select>
