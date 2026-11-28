<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, toast } from '@usehuxley/ui';
	import { ago } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	const PROVIDERS: Record<string, string> = {
		clamav: 'ClamAV',
		metadefender: 'OPSWAT MetaDefender'
	};
	const VIEWS = [
		{ key: 'infected', label: 'Quarantined' },
		{ key: 'failed', label: 'Couldn’t scan' },
		{ key: 'pending', label: 'Waiting' }
	] as const;
	const count = (key: string) =>
		key === 'infected'
			? data.status.infected
			: key === 'failed'
				? data.status.failed
				: data.status.pending;
	const sourceLabel = (s: string) => (s === 'receipt' ? 'Expense receipt' : 'Attachment');

	function formatSize(bytes: number) {
		if (bytes < 1024) return `${bytes} B`;
		if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
		return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
	}

	const said: SubmitFunction =
		() =>
		async ({ result, update }) => {
			const back = (
				result.type === 'success' || result.type === 'failure' ? result.data : undefined
			) as { message?: string } | undefined;
			if (back?.message)
				toast.push({ title: back.message, tone: result.type === 'success' ? 'success' : 'danger' });
			await update({ reset: false });
		};
</script>

<svelte:head>
	<title>Malware scanning · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Malware scanning"
		description="Uploaded files and email attachments are checked before anyone can download them. The scanner is set in the server's configuration (uploads.scan)."
	>
		{#if data.status.provider === 'off'}
			<p class="text-sm text-fg-muted">
				Scanning is off: uploads are available at once, unscanned. To turn it on, set
				<code class="font-mono text-xs">uploads.scan.provider</code> to ClamAV or MetaDefender.
			</p>
		{:else}
			<dl class="grid gap-x-6 gap-y-2 text-sm sm:grid-cols-[max-content_1fr]" aria-label="Scanner">
				<dt class="text-fg-muted">Scanner</dt>
				<dd class="flex flex-wrap items-center gap-2 text-fg">
					{PROVIDERS[data.status.provider] ?? data.status.provider}
					{#if data.status.reachable}
						<Badge tone="success">Answering</Badge>
					{:else}
						<Badge tone="danger">Not answering</Badge>
					{/if}
				</dd>
				{#if data.status.detail}
					<dt class="text-fg-muted">{data.status.reachable ? 'Version' : 'Problem'}</dt>
					<dd class={data.status.reachable ? 'text-fg' : 'text-danger'}>{data.status.detail}</dd>
				{/if}
				<dt class="text-fg-muted">Files not yet scanned</dt>
				<dd class="text-fg">
					{data.status.holdsUnscanned
						? 'Held until they’re scanned clean'
						: 'Can be downloaded until a scan finds something'}
				</dd>
			</dl>
		{/if}
	</Card>

	<Card title="Files">
		<nav class="mb-4 flex flex-wrap gap-2" aria-label="Show">
			{#each VIEWS as v (v.key)}
				<a
					href="{resolve('/(app)/settings/file-scans')}?show={v.key}"
					aria-current={data.view === v.key ? 'page' : undefined}
					class="rounded-control px-3 py-1.5 text-sm {data.view === v.key
						? 'bg-accent-soft font-medium text-accent'
						: 'text-fg-muted hover:bg-surface-sunken hover:text-fg'}"
				>
					{v.label} ({count(v.key).toLocaleString()})
				</a>
			{/each}
		</nav>
		{#if data.files.length === 0}
			<p class="text-sm text-fg-muted">
				{data.view === 'infected'
					? 'Nothing has been quarantined.'
					: data.view === 'failed'
						? 'Every file could be scanned.'
						: 'Nothing is waiting for a scan.'}
			</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Scanned files">
				{#each data.files as file (file.id)}
					<li class="flex flex-wrap items-start justify-between gap-3 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<p class="text-sm font-medium text-fg">{file.filename}</p>
							<p class="mt-0.5 text-sm text-fg-muted">
								{sourceLabel(file.source)} · {file.orgName} · {formatSize(file.sizeBytes)} · uploaded
								{ago(file.createdAt)}
							</p>
							{#if file.finding}
								<p class="mt-0.5 text-sm text-danger">Found: {file.finding}</p>
							{:else if file.lastError}
								<p class="mt-0.5 text-sm text-warning">
									{file.attempts}
									{file.attempts === 1 ? 'try' : 'tries'}: {file.lastError}
								</p>
							{/if}
						</div>
						<div class="flex items-center gap-2">
							{#if data.status.provider !== 'off'}
								<form method="POST" action="?/rescan" use:enhance={said}>
									<input type="hidden" name="id" value={file.id} />
									<Button type="submit" size="sm">Scan again</Button>
								</form>
							{/if}
							<form method="POST" action="?/remove" use:enhance={said}>
								<input type="hidden" name="id" value={file.id} />
								<Button
									type="submit"
									size="sm"
									variant="danger"
									aria-label="Delete {file.filename}"
								>
									Delete
								</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
