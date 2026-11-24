<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	const view = $derived(data.view);
	let busy = $state(false);

	// Back to the app, with the code or the refusal.
	$effect(() => {
		if (form && 'redirectTo' in form && form.redirectTo) window.location.assign(form.redirectTo);
	});
</script>

<svelte:head><title>Connect {view.clientName} · Huxley</title></svelte:head>

<div class="mx-auto flex max-w-lg flex-col gap-6 py-8">
	<Card title="Connect {view.clientName}?">
		<div class="flex flex-col gap-4 text-sm text-fg">
			<p>
				<strong>{view.clientName}</strong> wants to use Huxley as you, through Huxley's MCP server.
			</p>
			{#if view.fullAccess}
				<p>
					It can do <strong>anything your roles allow</strong>: read and change workflows, run
					them, and use integrations and data you can see. It can't do more than you can.
				</p>
			{:else}
				<div>
					<p>It can, within what your roles allow:</p>
					<ul class="mt-2 list-disc pl-5" aria-label="Requested permissions">
						{#each view.scopes as scope (scope.key)}
							<li>{scope.description} <code class="text-fg-muted">{scope.key}</code></li>
						{/each}
					</ul>
				</div>
			{/if}
			<p class="text-fg-muted">
				Afterwards you'll go back to <strong class="text-fg">{view.redirectHost}</strong>.
				The app chose its own name, so only approve this if you just started connecting it
				yourself. You can disconnect it at any time from your profile.
			</p>
			{#if form && 'message' in form && form.message}
				<p role="alert" class="text-danger">{form.message}</p>
			{/if}
			{#if form && 'redirectTo' in form && form.redirectTo}
				<p>
					{form.approved ? 'Connected.' : 'Cancelled.'} Returning you to the app…
					<a class="text-accent underline" href={form.redirectTo}>Continue</a>
				</p>
			{:else}
				<div class="flex justify-end gap-3">
					<form
						method="POST"
						action="{data.query}{data.query ? '&' : '?'}/deny"
						use:enhance={() => {
							busy = true;
							return ({ update }) => update({ reset: false });
						}}
					>
						<Button type="submit" disabled={busy}>Cancel</Button>
					</form>
					<form
						method="POST"
						action="{data.query}{data.query ? '&' : '?'}/approve"
						use:enhance={() => {
							busy = true;
							return ({ update }) => update({ reset: false });
						}}
					>
						<Button type="submit" variant="primary" disabled={busy}>Connect</Button>
					</form>
				</div>
			{/if}
		</div>
	</Card>
</div>
