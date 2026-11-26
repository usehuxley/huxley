<script lang="ts">
	import { resolve } from '$app/paths';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
</script>

<svelte:head>
	<title>Sign in · {data.branding.name}</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div
		class="w-full max-w-sm rounded-card border border-border bg-surface p-6 text-center shadow-card"
	>
		<h1 class="text-xl font-semibold text-fg">Sign in to {data.branding.name}</h1>
		<p class="mt-3 text-sm text-danger" role="alert">{data.message}</p>
		<p class="mt-5 text-sm">
			<a href={resolve('/(auth)/login')} class="text-accent hover:underline">Back to sign in</a>
		</p>
	</div>
</main>
