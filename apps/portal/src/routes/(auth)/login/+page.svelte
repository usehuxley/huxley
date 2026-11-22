<script lang="ts">
	import { Button, Field, Input, toast } from '@usehuxley/ui';

	let email = $state('');

	// Placeholder: portal sign-in (magic link, password + MFA, or the client's own
	// identity provider) arrives with the portal in Phase 7.
	function onsubmit(event: SubmitEvent) {
		event.preventDefault();
		toast.push({
			title: 'Sign-in is not available yet',
			description: 'The client portal opens in Phase 7.',
			tone: 'warning'
		});
	}
</script>

<svelte:head>
	<title>Sign in · Support portal</title>
</svelte:head>

<main class="flex min-h-screen items-center justify-center bg-bg px-4">
	<div class="w-full max-w-sm">
		<div class="mb-8 text-center">
			<h1 class="text-xl font-semibold text-fg">Sign in to your support portal</h1>
			<p class="mt-1 text-sm text-fg-muted">We'll email you a secure sign-in link.</p>
		</div>

		<div class="rounded-card border border-border bg-surface p-6 shadow-card">
			<form class="flex flex-col gap-4" {onsubmit}>
				<Field label="Work email" for="email">
					<Input id="email" type="email" autocomplete="email" required bind:value={email} />
				</Field>
				<Button type="submit" variant="primary" block>Email me a sign-in link</Button>
			</form>

			<div class="my-5 flex items-center gap-3 text-xs text-fg-subtle">
				<span class="h-px flex-1 bg-border"></span>or<span class="h-px flex-1 bg-border"></span>
			</div>

			<Button block disabled>Sign in with your organization</Button>
		</div>
	</div>
</main>
