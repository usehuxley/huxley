<script lang="ts">
	import { enhance } from '$app/forms';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	const timeFormat = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short'
	});
</script>

<svelte:head>
	<title>Notifications · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<div class="flex items-end justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold text-fg">Notifications</h1>
			<p class="mt-1 text-fg-muted">{data.feed.unreadCount} unread</p>
		</div>
		{#if data.feed.unreadCount > 0}
			<form method="POST" action="?/readAll" use:enhance>
				<Button type="submit" size="sm">Mark all read</Button>
			</form>
		{/if}
	</div>

	<Card title="Recent">
		{#if data.feed.items.length === 0}
			<p class="text-sm text-fg-muted">Nothing yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.feed.items as n (n.id)}
					<li class="flex items-start justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-medium text-fg">
								{#if !n.readAt}<span class="size-2 rounded-full bg-accent" aria-label="Unread"
									></span>{/if}
								{n.title}
							</p>
							{#if n.body}<p class="mt-0.5 text-sm text-fg-muted">{n.body}</p>{/if}
							<p class="mt-0.5 text-xs text-fg-subtle">
								{timeFormat.format(new Date(n.createdAt))}
							</p>
						</div>
						{#if !n.readAt}
							<form method="POST" action="?/read" use:enhance>
								<input type="hidden" name="id" value={n.id} />
								<Button type="submit" size="sm" variant="ghost" aria-label="Mark “{n.title}” read">
									Mark read
								</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card title="How you're notified" description="Security notices always come both ways.">
		<ul class="flex flex-col divide-y divide-border">
			{#each data.preferences as pref (pref.kind)}
				<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
					<p class="flex items-center gap-2 text-sm text-fg">
						{pref.description}
						{#if pref.mandatory}<Badge>Always on</Badge>{/if}
					</p>
					<form
						method="POST"
						action="?/preference"
						class="flex items-center gap-4 text-sm"
						use:enhance
					>
						<input type="hidden" name="kind" value={pref.kind} />
						<label class="flex items-center gap-1.5">
							<input
								type="checkbox"
								name="inApp"
								checked={pref.inApp}
								disabled={pref.mandatory}
								onchange={(e) => e.currentTarget.form?.requestSubmit()}
							/> In Huxley
						</label>
						<label class="flex items-center gap-1.5">
							<input
								type="checkbox"
								name="email"
								checked={pref.email}
								disabled={pref.mandatory}
								onchange={(e) => e.currentTarget.form?.requestSubmit()}
							/> Email
						</label>
					</form>
				</li>
			{/each}
		</ul>
	</Card>
</div>
