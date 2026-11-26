<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let withPassword = $state(false);
	let message = $derived(form && 'message' in form ? form.message : null);
	let sent = $derived(form && 'sent' in form ? form.sent : null);
</script>

<svelte:head>
	<title>Sign in · Support portal</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div class="w-full max-w-sm">
		<div class="mb-8 text-center">
			<h1 class="text-xl font-semibold text-fg">Sign in to your support portal</h1>
			<p class="mt-1 text-sm text-fg-muted">
				{withPassword ? 'With your email and password.' : 'We’ll email you a secure sign-in link.'}
			</p>
		</div>

		<div class="rounded-card border border-border bg-surface p-6 shadow-card">
			{#if sent}
				<p class="text-sm text-fg" role="status">
					If <strong>{sent}</strong> has access to the portal, a sign-in link is on its way. It works
					once, for 15 minutes.
				</p>
			{:else if withPassword}
				<form method="POST" action="?/password" class="flex flex-col gap-4" use:enhance>
					<input type="hidden" name="next" value={data.next} />
					<Field label="Work email" for="email">
						<Input id="email" name="email" type="email" autocomplete="email" required />
					</Field>
					<Field label="Password" for="password">
						<Input
							id="password"
							name="password"
							type="password"
							autocomplete="current-password"
							required
						/>
					</Field>
					<Button type="submit" variant="primary" block>Sign in</Button>
				</form>
			{:else}
				<form method="POST" action="?/link" class="flex flex-col gap-4" use:enhance>
					<Field label="Work email" for="email">
						<Input id="email" name="email" type="email" autocomplete="email" required />
					</Field>
					<Button type="submit" variant="primary" block>Email me a sign-in link</Button>
				</form>
			{/if}
			{#if message}<p class="mt-3 text-sm text-danger" role="alert">{message}</p>{/if}
			{#if !sent}
				<p class="mt-5 text-center text-sm">
					<button
						type="button"
						class="text-accent hover:underline"
						onclick={() => (withPassword = !withPassword)}
					>
						{withPassword ? 'Email me a link instead' : 'Sign in with a password'}
					</button>
				</p>
			{/if}
		</div>
	</div>
</main>
