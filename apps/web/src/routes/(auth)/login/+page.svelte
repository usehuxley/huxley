<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { form }: PageProps = $props();
	let submitting = $state(false);
</script>

<svelte:head>
	<title>Sign in · Huxley</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div class="w-full max-w-sm">
		<div class="mb-8 flex flex-col items-center gap-3 text-center">
			<span
				class="flex size-11 items-center justify-center rounded-card bg-accent text-xl font-bold text-accent-fg"
				aria-hidden="true"
			>
				H
			</span>
			<h1 class="text-xl font-semibold text-fg">Sign in to Huxley</h1>
		</div>

		<div class="rounded-card border border-border bg-surface p-6 shadow-card">
			<form
				method="POST"
				class="flex flex-col gap-4"
				use:enhance={() => {
					submitting = true;
					return async ({ update }) => {
						await update({ reset: false });
						submitting = false;
					};
				}}
			>
				{#if form?.message}
					<p
						role="alert"
						class="rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger"
					>
						{form.message}
					</p>
				{/if}
				<Field label="Email" for="email">
					<Input
						id="email"
						name="email"
						type="email"
						autocomplete="username"
						required
						value={form?.email ?? ''}
					/>
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
				<Button type="submit" variant="primary" block disabled={submitting}>
					{submitting ? 'Signing in…' : 'Sign in'}
				</Button>
			</form>

			<div class="my-5 flex items-center gap-3 text-xs text-fg-subtle">
				<span class="h-px flex-1 bg-border"></span>or<span class="h-px flex-1 bg-border"></span>
			</div>

			<Button block disabled title="Single sign-on arrives later in Phase 1">
				Sign in with Microsoft
			</Button>
		</div>
	</div>
</main>
