<script lang="ts">
	import { resolve } from '$app/paths';
	import { Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Sign-in · Integrations · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-xl flex-col gap-6">
	<h1 class="text-2xl font-semibold text-fg">Connection sign-in</h1>
	<Card title="It didn't work">
		<p class="text-sm text-fg" role="alert">{data.message}</p>
		<p class="mt-3 text-sm">
			<a href={resolve('/(app)/integrations')} class="text-accent hover:underline">
				Back to integrations
			</a>
			to try again.
		</p>
	</Card>
</div>
