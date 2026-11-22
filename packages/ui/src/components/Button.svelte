<script lang="ts" module>
	export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger';
	export type ButtonSize = 'sm' | 'md' | 'lg';
</script>

<script lang="ts">
	import type { Snippet } from 'svelte';
	import type { HTMLAnchorAttributes, HTMLButtonAttributes } from 'svelte/elements';

	type Common = {
		variant?: ButtonVariant;
		size?: ButtonSize;
		/** Stretch to the container width. */
		block?: boolean;
		class?: string;
		children: Snippet;
	};
	type Props = Common &
		((HTMLButtonAttributes & { href?: undefined }) | (HTMLAnchorAttributes & { href: string }));

	let {
		variant = 'secondary',
		size = 'md',
		block = false,
		class: className = '',
		children,
		...rest
	}: Props = $props();

	const variants: Record<ButtonVariant, string> = {
		primary: 'bg-accent text-accent-fg hover:bg-accent-hover border-transparent',
		secondary: 'bg-surface text-fg border-border-strong hover:bg-surface-sunken',
		ghost: 'bg-transparent text-fg border-transparent hover:bg-surface-sunken',
		danger: 'bg-danger text-white border-transparent hover:opacity-90'
	};
	const sizes: Record<ButtonSize, string> = {
		sm: 'h-8 px-3 text-sm gap-1.5',
		md: 'h-9 px-4 text-sm gap-2',
		lg: 'h-11 px-5 text-base gap-2'
	};

	let classes = $derived(
		[
			'inline-flex items-center justify-center rounded-control border font-medium',
			'transition-colors disabled:pointer-events-none disabled:opacity-50',
			variants[variant],
			sizes[size],
			block ? 'w-full' : '',
			className
		].join(' ')
	);
</script>

{#if rest.href !== undefined}
	<a class={classes} {...rest as HTMLAnchorAttributes}>{@render children()}</a>
{:else}
	<button type="button" class={classes} {...rest as HTMLButtonAttributes}>
		{@render children()}
	</button>
{/if}
