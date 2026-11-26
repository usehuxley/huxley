<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let setup = $derived(form && 'setup' in form ? form.setup : undefined);
	let codes = $derived(form && 'recoveryCodes' in form ? form.recoveryCodes : undefined);
	let message = $derived(form && 'message' in form ? form.message : undefined);
</script>

<svelte:head>
	<title>Two-factor sign-in · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<h1 class="text-2xl font-semibold text-fg">Two-factor sign-in</h1>
	{#if data.me.mfaSetupRequired && !codes}
		<p
			class="rounded-card border border-warning/40 bg-warning/10 p-3 text-sm text-fg"
			role="status"
		>
			Your organization requires two-factor sign-in. Set it up to continue.
		</p>
	{/if}

	{#if codes}
		<Card
			title="Your recovery codes"
			description="Keep them somewhere safe. Each signs you in once if you lose your phone. They won’t be shown again."
		>
			<ul class="grid grid-cols-2 gap-2 font-mono text-sm text-fg" aria-label="Recovery codes">
				{#each codes as c (c)}<li>{c}</li>{/each}
			</ul>
			<a href={resolve('/(portal)')} class="mt-4 inline-block text-sm text-accent hover:underline">
				Done
			</a>
		</Card>
	{:else if data.mfa.enabled}
		<Card
			title="On"
			description="You’re asked for a code from your authenticator app when you sign in."
		>
			<p class="text-sm text-fg-muted">
				{data.mfa.recoveryCodesLeft} recovery codes left.
			</p>
			<form method="POST" action="?/regenerate" class="mt-3" use:enhance>
				<Button type="submit" variant="secondary" size="sm">Replace recovery codes</Button>
			</form>
		</Card>
		{#if !data.mfa.required}
			<Card title="Turn it off" description="Enter a current code from your app.">
				<form method="POST" action="?/disable" class="flex flex-wrap items-end gap-3" use:enhance>
					<Field label="Code" for="off-code">
						<Input id="off-code" name="code" autocomplete="one-time-code" required class="w-40" />
					</Field>
					<Button type="submit" variant="danger">Turn off</Button>
				</form>
			</Card>
		{/if}
	{:else if setup}
		<Card
			title="Scan this with your authenticator app"
			description="Microsoft Authenticator, Google Authenticator, 1Password and others work."
		>
			<div class="flex flex-col gap-4 sm:flex-row sm:items-start">
				<!-- eslint-disable-next-line svelte/no-at-html-tags -- a QR code SVG we rendered -->
				<div class="w-44 shrink-0 rounded bg-white p-2" aria-label="QR code">{@html setup.qr}</div>
				<div class="flex flex-col gap-3">
					<p class="text-sm text-fg-muted">
						Or type this key: <code class="font-mono break-all text-fg">{setup.secret}</code>
					</p>
					<form method="POST" action="?/confirm" class="flex flex-wrap items-end gap-3" use:enhance>
						<input type="hidden" name="secret" value={setup.secret} />
						<input type="hidden" name="qr" value={setup.qr} />
						<Field label="Code from the app" for="setup-code">
							<Input
								id="setup-code"
								name="code"
								autocomplete="one-time-code"
								inputmode="numeric"
								required
								class="w-40"
							/>
						</Field>
						<Button type="submit" variant="primary">Turn on</Button>
					</form>
				</div>
			</div>
		</Card>
	{:else}
		<Card
			title={form && 'disabled' in form ? 'Turned off' : 'Off'}
			description="Add a code from an authenticator app to signing in with a link or password."
		>
			<form method="POST" action="?/begin" use:enhance>
				<Button type="submit" variant="primary">Set up an authenticator app</Button>
			</form>
		</Card>
	{/if}
	{#if message}<p class="text-sm text-danger" role="alert">{message}</p>{/if}
</div>
