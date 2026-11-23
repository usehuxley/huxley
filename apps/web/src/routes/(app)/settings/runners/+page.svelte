<script lang="ts">
	import { enhance } from '$app/forms';
	import { Badge, Button, Card, EmptyState, Field, Input } from '@usehuxley/ui';
	import { Server } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	const when = (v: string | null | undefined) => (v ? new Date(v).toLocaleString() : 'never');
</script>

<svelte:head>
	<title>Script runners · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<header>
		<h2 class="text-xl font-semibold text-fg">Script runners</h2>
		<p class="mt-1 text-sm text-fg-muted">
			Runners run scripts in sandboxes and connect to Huxley with a client certificate.
		</p>
	</header>
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-fg" role="status">{form.message}</p>
	{/if}

	<Card title="Runners">
		{#if data.runners.length === 0}
			<EmptyState title="No runners yet" description="Enrol one below.">
				{#snippet icon()}<Server size={20} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Runners">
				{#each data.runners as runner (runner.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0 text-sm">
							<p class="font-medium text-fg">
								{runner.name}
								{#if runner.revokedAt}<Badge tone="neutral">Revoked</Badge>{/if}
								<Badge tone="info">{runner.source === 'huxley' ? 'Enrolled' : 'Your CA'}</Badge>
							</p>
							<p class="text-xs text-fg-muted">
								Last seen {when(runner.lastSeenAt)}
								{#if runner.languages.length}· {runner.languages.join(', ')}{/if}
								{#if runner.version}· {runner.version}{/if}
								{#if runner.certExpiresAt}· certificate until {when(runner.certExpiresAt)}{/if}
							</p>
						</div>
						{#if !runner.revokedAt}
							<form method="POST" action="?/revoke" use:enhance>
								<input type="hidden" name="id" value={runner.id} />
								<Button type="submit" variant="danger" size="sm">Revoke</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card
		title="Enrol a runner"
		description="Makes a single-use token, valid for 24 hours. The runner trades it for its certificate."
	>
		{#if form && 'token' in form && form.token}
			<div class="flex flex-col gap-2 text-sm" role="status">
				<p class="text-fg">Token for <strong>{form.name}</strong>, shown once:</p>
				<pre
					class="overflow-x-auto rounded-control bg-surface-sunken p-2 font-mono text-xs"
					data-testid="runner-token">{form.token}</pre>
				<p class="text-fg-muted">On the runner's host:</p>
				<pre
					class="overflow-x-auto rounded-control bg-surface-sunken p-2 font-mono text-xs">huxley runner enroll --server {data.server} --token {form.token}
huxley runner run</pre>
			</div>
		{:else}
			<form method="POST" action="?/enroll" use:enhance class="flex flex-wrap items-end gap-3">
				<Field
					label="Runner name"
					for="enroll-name"
					hint="Lowercase letters, digits, dots and hyphens"
				>
					<Input id="enroll-name" name="name" required maxlength={63} />
				</Field>
				<Button type="submit">Create token</Button>
			</form>
		{/if}
	</Card>

	<Card
		title="Use your own CA"
		description="Runners with certificates from the CA in runners.client_ca_file are accepted once registered here under their certificate's common name."
	>
		<form method="POST" action="?/external" use:enhance class="flex flex-wrap items-end gap-3">
			<Field label="Certificate common name" for="external-name">
				<Input id="external-name" name="name" required maxlength={63} />
			</Field>
			<Button type="submit" variant="secondary">Register</Button>
		</form>
	</Card>
</div>
