<script lang="ts">
	import { resolve } from '$app/paths';
	import { Button } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Sign in · {data.branding.name}</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div
		class="w-full max-w-sm rounded-card border border-border bg-surface p-6 text-center shadow-card"
	>
		<h1 class="text-xl font-semibold text-fg">Sign in to {data.branding.name}</h1>
		{#if data.token}
			<form method="POST" class="mt-5">
				<input type="hidden" name="token" value={data.token} />
				<Button type="submit" variant="primary" block>Sign in</Button>
			</form>
		{:else}
			<p class="mt-3 text-sm text-fg-muted">This link is incomplete. Ask for a new one.</p>
		{/if}
		{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
		<p class="mt-5 text-sm">
			<a href={resolve('/(auth)/login')} class="text-accent hover:underline">Get a new link</a>
		</p>
	</div>
</main>
