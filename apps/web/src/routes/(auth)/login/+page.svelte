<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { page } from '$app/state';
	import { Button, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let submitting = $state(false);
	let useRecoveryCode = $state(false);

	// The code step shows after a correct password, and again on reload while
	// the challenge cookie lasts, until it expires or the user starts over.
	let mfaStep = $derived(
		Boolean(form && 'mfa' in form && form.mfa) ||
			(data.mfaPending && !(form && ('expired' in form || 'restarted' in form)))
	);

	/** A named action that keeps `?next=` so sign-in returns to the right page. */
	function action(name: string): string {
		const next = page.url.searchParams.get('next');
		return next ? `?/${name}&next=${encodeURIComponent(next)}` : `?/${name}`;
	}

	const withPending: SubmitFunction = () => {
		submitting = true;
		return async ({ update }) => {
			await update({ reset: false });
			submitting = false;
		};
	};
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
			{#if form?.message}
				<p
					role="alert"
					class="mb-4 rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger"
				>
					{form.message}
				</p>
			{/if}

			{#if mfaStep}
				<form
					method="POST"
					action={action('mfa')}
					class="flex flex-col gap-4"
					use:enhance={withPending}
				>
					{#if useRecoveryCode}
						<Field
							label="Recovery code"
							for="recoveryCode"
							hint="One of the codes you saved when you set up your authenticator. Each works once."
						>
							<Input
								id="recoveryCode"
								name="recoveryCode"
								autocomplete="off"
								autocapitalize="characters"
								spellcheck={false}
								required
							/>
						</Field>
					{:else}
						<Field
							label="Authentication code"
							for="code"
							hint="The six-digit code from your authenticator app."
						>
							<Input
								id="code"
								name="code"
								inputmode="numeric"
								autocomplete="one-time-code"
								pattern="[0-9 ]*"
								maxlength={7}
								required
							/>
						</Field>
					{/if}
					<Button type="submit" variant="primary" block disabled={submitting}>
						{submitting ? 'Verifying…' : 'Verify'}
					</Button>
				</form>
				<div class="mt-4 flex items-center justify-between text-sm">
					<button
						type="button"
						class="text-accent hover:underline"
						onclick={() => (useRecoveryCode = !useRecoveryCode)}
					>
						{useRecoveryCode ? 'Use authenticator app' : 'Use a recovery code'}
					</button>
					<form method="POST" action={action('restart')} use:enhance>
						<button type="submit" class="text-fg-muted hover:underline">Start over</button>
					</form>
				</div>
			{:else}
				<form
					method="POST"
					action={action('password')}
					class="flex flex-col gap-4"
					use:enhance={withPending}
				>
					<Field label="Email" for="email">
						<Input
							id="email"
							name="email"
							type="email"
							autocomplete="username"
							required
							value={form && 'email' in form && typeof form.email === 'string' ? form.email : ''}
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
			{/if}

			<div class="my-5 flex items-center gap-3 text-xs text-fg-subtle">
				<span class="h-px flex-1 bg-border"></span>or<span class="h-px flex-1 bg-border"></span>
			</div>

			<Button block disabled title="Single sign-on arrives later in Phase 1">
				Sign in with Microsoft
			</Button>
		</div>
	</div>
</main>
