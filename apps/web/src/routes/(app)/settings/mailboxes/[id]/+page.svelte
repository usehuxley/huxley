<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { Badge, Button, Card, toast } from '@usehuxley/ui';
	import MailboxForm from '#lib/mail/MailboxForm.svelte';
	import { OUTCOMES } from '#lib/mail/form.ts';
	import { ago } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let mailbox = $derived(data.mailbox);
	let errors: Record<string, string> = $derived(form?.errors ?? {});
	let postUrl = $derived(`${page.url.origin}/api/v1/mailboxes/${mailbox.id}/messages`);

	const saved: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') toast.push({ title: 'Mailbox saved.', tone: 'success' });
			else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>{mailbox.name} · Mailboxes · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<div>
		<a href={resolve('/(app)/settings/mailboxes')} class="text-sm text-fg-muted hover:text-fg">
			← Mailboxes
		</a>
		<h2 class="mt-2 text-xl font-semibold text-fg">{mailbox.name}</h2>
		<p class="text-sm text-fg-muted">{mailbox.address}</p>
		{#if mailbox.lastError}
			<p role="alert" class="mt-2 text-sm text-danger">
				Last check failed: {mailbox.lastError}
			</p>
		{:else if mailbox.lastCheckedAt}
			<p class="mt-2 text-sm text-fg-muted">Checked {ago(mailbox.lastCheckedAt)}.</p>
		{/if}
	</div>

	<Card title="Recent email" description="What arrived, and what became of it.">
		{#if data.emails.length === 0}
			<p class="text-sm text-fg-muted">Nothing yet.</p>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-left text-sm" aria-label="Recent email">
					<thead class="text-xs tracking-wide text-fg-muted uppercase">
						<tr>
							<th scope="col" class="py-2 pr-4 font-medium">Received</th>
							<th scope="col" class="py-2 pr-4 font-medium">From</th>
							<th scope="col" class="py-2 pr-4 font-medium">Subject</th>
							<th scope="col" class="py-2 font-medium">Outcome</th>
						</tr>
					</thead>
					<tbody>
						{#each data.emails as email (email.id)}
							{@const outcome = email.outcome ? OUTCOMES[email.outcome] : null}
							<tr class="border-t border-border align-top">
								<td class="py-2 pr-4 whitespace-nowrap text-fg-muted">
									<time datetime={email.receivedAt}>{ago(email.receivedAt)}</time>
								</td>
								<td class="max-w-48 truncate py-2 pr-4">{email.fromName ?? email.fromAddress}</td>
								<td class="max-w-72 truncate py-2 pr-4">{email.subject || '(no subject)'}</td>
								<td class="py-2">
									{#if outcome}
										<Badge tone={outcome.tone}>{outcome.label}</Badge>
										{#if email.ticketId}
											<a
												href={resolve('/(app)/tickets/[id]', { id: email.ticketId })}
												class="ml-1 text-accent hover:underline"
											>
												{email.outcomeDetail?.replace(/^(opened|added to) /, '')}
											</a>
										{:else if email.outcomeDetail}
											<span class="ml-1 text-fg-muted">{email.outcomeDetail}</span>
										{/if}
									{:else}
										<Badge>Waiting</Badge>
									{/if}
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</Card>

	{#if mailbox.kind === 'forward'}
		<Card
			title="Forwarding"
			description="Post each message to this address, as a mail server's forwarding rule or a mail service's inbound webhook would. Use a service account with mailbox:ingest."
		>
			<pre class="overflow-x-auto rounded-control bg-surface-sunken p-3 text-xs"><code
					>curl -X POST {postUrl} \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: message/rfc822" \
  --data-binary @message.eml</code
				></pre>
		</Card>
	{/if}

	<Card title="Settings">
		<form method="POST" action="?/save" class="flex flex-col gap-4" use:enhance={saved}>
			<MailboxForm {mailbox} {errors} types={data.types} queues={data.queues} orgs={data.orgs} />
			<div class="flex justify-between gap-2">
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</form>
	</Card>

	<Card
		title="Archive"
		description="An archived mailbox takes no more mail. Its email and tickets stay."
	>
		<form method="POST" action="?/archive" use:enhance>
			<Button type="submit" variant="danger">Archive mailbox</Button>
		</form>
	</Card>
</div>
