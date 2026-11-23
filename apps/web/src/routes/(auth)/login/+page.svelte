<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { onMount } from 'svelte';
	import { Button, Field, Input } from '@usehuxley/ui';
	import { isCancelled, passkeysSupported, postJson, usePasskey } from '#lib/passkey.ts';
	import type { PageProps } from './$types';

	type Method = 'totp' | 'passkey' | 'recoveryCode';

	let { data, form }: PageProps = $props();
	let submitting = $state(false);
	let useRecoveryCode = $state(false);
	let supported = $state(false);
	/** Errors from the passkey buttons, which don't go through form actions. */
	let passkeyError = $state<string | null>(null);
	/** Set when a passkey call reports the challenge expired. */
	let passkeyExpired = $state(false);

	onMount(() => {
		supported = passkeysSupported();
	});

	// The second-factor step shows after a correct password, and again on
	// reload while the challenge cookie lasts, until it expires or the user
	// starts over.
	let methods = $derived.by((): Method[] | null => {
		if (passkeyExpired) return null;
		if (form && 'methods' in form && Array.isArray(form.methods)) return form.methods;
		if (form && ('expired' in form || 'restarted' in form)) return null;
		return data.mfaMethods;
	});
	let has = (m: Method) => methods?.includes(m) ?? false;
	let message = $derived(passkeyError ?? (form?.message ? String(form.message) : null));

	let next = $derived(page.url.searchParams.get('next'));

	/** A named action that keeps `?next=` so sign-in returns to the right page. */
	function action(name: string): string {
		return next ? `?/${name}&next=${encodeURIComponent(next)}` : `?/${name}`;
	}

	const withPending: SubmitFunction = () => {
		submitting = true;
		passkeyError = null;
		return async ({ update }) => {
			await update({ reset: false });
			submitting = false;
		};
	};

	/** Runs a passkey ceremony against `base` (`…/options` then `…`). */
	async function passkey(base: string) {
		submitting = true;
		passkeyError = null;
		try {
			const options = await postJson<{ ceremony: string; options: never; message?: string }>(
				`${base}/options`
			);
			if (!options.ok) {
				passkeyError = options.data.message ?? 'Passkey sign-in is unavailable right now.';
				return;
			}
			const credential = await usePasskey(options.data.options);
			const result = await postJson<{ location?: string; message?: string; expired?: boolean }>(
				base,
				{ ceremony: options.data.ceremony, credential, next }
			);
			if (result.ok && result.data.location) {
				// eslint-disable-next-line svelte/no-navigation-without-resolve -- validated same-site path from the server
				await goto(result.data.location, { invalidateAll: true });
				return;
			}
			passkeyExpired = Boolean(result.data.expired);
			passkeyError = result.data.message ?? "That passkey didn't work.";
		} catch (error) {
			if (!isCancelled(error)) passkeyError = "Your browser couldn't use a passkey.";
		} finally {
			submitting = false;
		}
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
			{#if message}
				<p
					role="alert"
					class="mb-4 rounded-control border border-danger/30 bg-danger-soft px-3 py-2 text-sm text-danger"
				>
					{message}
				</p>
			{/if}

			{#if methods}
				<p class="mb-4 text-sm text-fg-muted">Confirm it's you with your second factor.</p>
				<div class="flex flex-col gap-4">
					{#if has('passkey') && supported && !useRecoveryCode}
						<Button
							variant="primary"
							block
							disabled={submitting}
							onclick={() => passkey('/login/mfa-passkey')}
						>
							Use a passkey
						</Button>
					{/if}
					{#if useRecoveryCode || has('totp')}
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
									hint="One of the codes you saved when you set up two-factor authentication. Each works once."
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
							<Button
								type="submit"
								variant={has('passkey') && supported && !useRecoveryCode ? 'secondary' : 'primary'}
								block
								disabled={submitting}
							>
								{submitting ? 'Verifying…' : 'Verify'}
							</Button>
						</form>
					{/if}
				</div>
				<div class="mt-4 flex items-center justify-between text-sm">
					{#if has('recoveryCode')}
						<button
							type="button"
							class="text-accent hover:underline"
							onclick={() => (useRecoveryCode = !useRecoveryCode)}
						>
							{useRecoveryCode ? 'Back' : 'Use a recovery code'}
						</button>
					{:else}
						<span></span>
					{/if}
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
							autocomplete="username webauthn"
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

				<div class="my-5 flex items-center gap-3 text-xs text-fg-subtle">
					<span class="h-px flex-1 bg-border"></span>or<span class="h-px flex-1 bg-border"></span>
				</div>

				<div class="flex flex-col gap-2">
					{#if data.passkeysEnabled && supported}
						<Button block disabled={submitting} onclick={() => passkey('/login/passkey')}>
							Sign in with a passkey
						</Button>
					{/if}
					<Button block disabled title="Single sign-on arrives later in Phase 1">
						Sign in with Microsoft
					</Button>
				</div>
			{/if}
		</div>
	</div>
</main>
