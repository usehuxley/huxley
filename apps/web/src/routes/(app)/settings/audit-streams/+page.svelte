<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { ago } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	type Stream = (typeof data.streams)[number];

	let open = $state(false);
	let editing = $state<Stream | null>(null);
	let kind = $state<'webhook' | 'splunk_hec'>('webhook');
	let errors: Record<string, string> = $derived(
		(form as { errors?: Record<string, string> } | null)?.errors ?? {}
	);

	function edit(stream: Stream | null) {
		editing = stream;
		kind = stream?.kind ?? 'webhook';
		open = true;
	}

	const kinds = [
		{ value: 'webhook', label: 'Webhook (NDJSON over HTTPS)' },
		{ value: 'splunk_hec', label: 'Splunk HTTP Event Collector' }
	];
	const kindLabel = (k: string) => (k === 'splunk_hec' ? 'Splunk HEC' : 'Webhook');

	const said: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const back = (
				result.type === 'success' || result.type === 'failure' ? result.data : undefined
			) as { message?: string; saved?: boolean; errors?: Record<string, string> } | undefined;
			if (back?.saved) open = false;
			if (result.type === 'failure' && back?.errors && Object.keys(back.errors).length > 0) return;
			if (back?.message)
				toast.push({ title: back.message, tone: result.type === 'success' ? 'success' : 'danger' });
		};
</script>

<svelte:head>
	<title>Audit streaming · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Audit streaming"
		description="Send the audit log to your SIEM as it's written: in order, with each record's hash so the chain can be checked there too. For syslog, Elastic or Sentinel, point a webhook at a forwarder such as Vector or Fluent Bit."
	>
		{#snippet actions()}
			<Button onclick={() => edit(null)}><Plus size={16} /> New stream</Button>
		{/snippet}
		{#if data.streams.length === 0}
			<p class="text-sm text-fg-muted">The audit log isn't streamed anywhere.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Audit streams">
				{#each data.streams as stream (stream.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<button
								type="button"
								class="text-sm font-medium text-fg hover:underline"
								onclick={() => edit(stream)}
							>
								{stream.name}
							</button>
							<p class="mt-0.5 truncate text-sm text-fg-muted">{stream.url}</p>
							{#if stream.lastError}
								<p class="mt-0.5 text-sm text-danger">
									{stream.failures} failed {stream.failures === 1 ? 'attempt' : 'attempts'}: {stream.lastError}
								</p>
							{/if}
						</div>
						<div class="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
							<Badge>{kindLabel(stream.kind)}</Badge>
							{#if !stream.enabled}
								<Badge tone="warning">Paused</Badge>
							{:else if stream.lastError}
								<Badge tone="danger">Failing</Badge>
							{/if}
							<span>
								{stream.behind === 0
									? 'Up to date'
									: `${stream.behind.toLocaleString()} ${stream.behind === 1 ? 'record' : 'records'} waiting`}{stream.lastShippedAt
									? ` · sent ${ago(stream.lastShippedAt)}`
									: ''}
							</span>
							<form method="POST" action="?/test" use:enhance={said}>
								<input type="hidden" name="id" value={stream.id} />
								<Button type="submit" size="sm">Send a test</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open title={editing ? editing.name : 'New stream'}>
	{#key editing?.id ?? 'new'}
		<form method="POST" action="?/save" class="flex flex-col gap-4" use:enhance={said}>
			{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
			<Field label="Name" for="as-name" error={errors.name}>
				<Input id="as-name" name="name" required value={editing?.name ?? ''} />
			</Field>
			<Field label="Destination" for="as-kind">
				<Select id="as-kind" name="kind" bind:value={kind} options={kinds} />
			</Field>
			<Field
				label="URL"
				for="as-url"
				error={errors.url}
				hint={kind === 'splunk_hec'
					? 'The collector, such as https://splunk.example.com:8088'
					: 'Records are POSTed here, one JSON object per line'}
			>
				<Input id="as-url" name="url" type="url" required value={editing?.url ?? ''} />
			</Field>
			{#if kind === 'webhook'}
				<Field
					label="Header"
					for="as-header"
					error={errors.headerName}
					hint="Where the secret goes"
				>
					<Input
						id="as-header"
						name="headerName"
						placeholder="Authorization"
						value={editing?.headerName ?? ''}
					/>
				</Field>
			{/if}
			<Field
				label={kind === 'splunk_hec' ? 'HEC token' : 'Header value'}
				for="as-secret"
				hint={editing?.hasSecret ? 'Set. Leave empty to keep it.' : 'Stored encrypted'}
			>
				<Input id="as-secret" name="secret" type="password" autocomplete="off" />
			</Field>
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="enabled" checked={editing?.enabled ?? true} />
				Sending
			</label>
			{#if !editing}
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="fromStart" />
					Send the whole audit log, not just what's recorded from now on
				</label>
			{/if}
			<div class="flex flex-wrap justify-between gap-2">
				{#if editing}
					<Button type="submit" formaction="?/delete" formnovalidate variant="danger">
						Remove
					</Button>
				{:else}
					<span></span>
				{/if}
				<div class="flex gap-2">
					<Button onclick={() => (open = false)}>Cancel</Button>
					<Button type="submit" variant="primary">{editing ? 'Save' : 'Add stream'}</Button>
				</div>
			</div>
		</form>
	{/key}
</Dialog>
