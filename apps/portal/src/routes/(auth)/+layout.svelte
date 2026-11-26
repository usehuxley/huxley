<script lang="ts">
	import { accentStyle } from '#lib/branding.ts';
	import type { LayoutProps } from './$types';

	let { data, children }: LayoutProps = $props();
</script>

<div style={accentStyle(data.branding.accentColor)}>
	{#if data.branding.logo}
		<div class="flex justify-center pt-10">
			<img src={data.branding.logo} alt={data.branding.name} class="h-10 max-w-48 object-contain" />
		</div>
	{/if}
	{@render children()}
</div>
