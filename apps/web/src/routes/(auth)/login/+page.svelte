<script lang="ts">
	import { Button, Field, Input, toast } from '@usehuxley/ui';

	let email = $state('');
	let password = $state('');

	// Placeholder until authentication lands in Phase 1 (local accounts, MFA, OIDC).
	function onsubmit(event: SubmitEvent) {
		event.preventDefault();
		toast.push({
			title: 'Sign-in is not available yet',
			description: 'Authentication arrives in Phase 1.',
			tone: 'warning'
		});
	}
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
			<form class="flex flex-col gap-4" {onsubmit}>
				<Field label="Email" for="email">
					<Input id="email" type="email" autocomplete="username" required bind:value={email} />
				</Field>
				<Field label="Password" for="password">
					<Input
						id="password"
						type="password"
						autocomplete="current-password"
						required
						bind:value={password}
					/>
				</Field>
				<Button type="submit" variant="primary" block>Sign in</Button>
			</form>

			<div class="my-5 flex items-center gap-3 text-xs text-fg-subtle">
				<span class="h-px flex-1 bg-border"></span>or<span class="h-px flex-1 bg-border"></span>
			</div>

			<Button block disabled>Sign in with Microsoft</Button>
		</div>
	</div>
</main>
