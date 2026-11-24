<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Select } from '@usehuxley/ui';
	import MailboxForm from '#lib/mail/MailboxForm.svelte';
	import { ago } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let adding = $state(false);
	let errors: Record<string, string> = $derived(form?.errors ?? {});

	const kindLabel = (kind: string) =>
		kind === 'imap' ? 'IMAP' : kind === 'graph' ? 'Microsoft 365' : 'Forwarded';
</script>

<svelte:head>
	<title>Mailboxes · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Mailboxes"
		description="Email to these addresses becomes tickets and replies. Install the Service desk email pack (Settings → Packs) for the workflows that fetch mail, open tickets and email your replies."
	>
		{#snippet actions()}
			<Button onclick={() => (adding = true)}><Plus size={16} /> New mailbox</Button>
		{/snippet}
		{#if data.mailboxes.length === 0}
			<p class="text-sm text-fg-muted">No mailboxes yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.mailboxes as mailbox (mailbox.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<a
								href={resolve('/(app)/settings/mailboxes/[id]', { id: mailbox.id })}
								class="text-sm font-medium text-fg hover:underline"
							>
								{mailbox.name}
							</a>
							<p class="mt-0.5 truncate text-sm text-fg-muted">{mailbox.address}</p>
							{#if mailbox.lastError}
								<p class="mt-0.5 text-sm text-danger">{mailbox.lastError}</p>
							{/if}
						</div>
						<div class="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
							<Badge>{kindLabel(mailbox.kind)}</Badge>
							{#if !mailbox.enabled}<Badge tone="warning">Off</Badge>{/if}
							{#if mailbox.kind === 'imap'}
								<span>
									{mailbox.lastCheckedAt
										? `Checked ${ago(mailbox.lastCheckedAt)}`
										: 'Not checked yet'}
								</span>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open={adding} title="New mailbox">
	<form method="POST" action="?/create" class="flex flex-col gap-4" use:enhance>
		{#if form?.message}<p role="alert" class="text-sm text-danger">{form.message}</p>{/if}
		{#if data.providers.length > 1}
			<Field label="Belongs to" for="mb-org">
				<Select
					id="mb-org"
					name="orgId"
					value={data.providers[0]?.value}
					options={data.providers}
				/>
			</Field>
		{:else}
			<input type="hidden" name="orgId" value={data.providers[0]?.value ?? ''} />
		{/if}
		<MailboxForm {errors} types={data.types} queues={data.queues} orgs={data.orgs} />
		<div class="flex justify-end gap-2">
			<Button onclick={() => (adding = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Add mailbox</Button>
		</div>
	</form>
</Dialog>
