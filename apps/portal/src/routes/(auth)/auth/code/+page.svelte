<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Sign in · {data.branding.name}</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div class="w-full max-w-sm rounded-card border border-border bg-surface p-6 shadow-card">
		<h1 class="text-xl font-semibold text-fg">Enter your code</h1>
		<p class="mt-1 text-sm text-fg-muted">
			From your authenticator app, or one of your recovery codes.
		</p>
		<form method="POST" class="mt-5 flex flex-col gap-4" use:enhance>
			<input type="hidden" name="next" value={data.next} />
			<Field label="Code" for="code">
				<Input
					id="code"
					name="code"
					autocomplete="one-time-code"
					inputmode="text"
					required
					autofocus
				/>
			</Field>
			<Button type="submit" variant="primary" block>Sign in</Button>
		</form>
		{#if form?.message}<p class="mt-3 text-sm text-danger" role="alert">{form.message}</p>{/if}
		<p class="mt-5 text-center text-sm">
			<a href={resolve('/(auth)/login')} class="text-accent hover:underline">Start again</a>
		</p>
	</div>
</main>
