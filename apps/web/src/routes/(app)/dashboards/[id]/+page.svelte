<script lang="ts">
	import { enhance } from '$app/forms';
	import { goto, invalidateAll } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { CalendarClock, Copy, Download, Pencil, RefreshCw, Send } from '@lucide/svelte';
	import { Badge, Button, Select } from '@usehuxley/ui';
	import { Widget } from '@usehuxley/widgets';
	import { RANGES } from '#lib/dashboards/spec.ts';
	import PublishDialog from '#lib/dashboards/PublishDialog.svelte';
	import ScheduleDialog from '#lib/dashboards/ScheduleDialog.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let publishing = $state(false);
	let scheduling = $state(false);
	let exportQuery = $derived(
		`range=${encodeURIComponent(data.range)}${data.orgId ? `&org=${encodeURIComponent(data.orgId)}` : ''}`
	);
	let pending = $derived(data.submissions.find((s) => s.status === 'pending'));
	let latest = $derived(data.submissions[0]);
	let names = (ids: string[], from: { id: string; name: string }[]) =>
		ids.map((id) => from.find((o) => o.id === id)?.name ?? 'Unknown').join(', ');
	let audience = $derived(
		[
			names(data.dashboard.audienceTeams, data.audiences.teams),
			names(data.dashboard.audienceRoles, data.audiences.roles)
		]
			.filter(Boolean)
			.join('; ')
	);

	function pick(name: 'range' | 'org', value: string) {
		const url = new URL(page.url.href);
		if (value) url.searchParams.set(name, value);
		else url.searchParams.delete(name);
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- the same page, new filters
		void goto(url);
	}
</script>

<svelte:head><title>{data.dashboard.name} · Huxley</title></svelte:head>

<div class="flex flex-col gap-4">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(app)/dashboards')} class="text-sm text-accent hover:underline">
				Dashboards
			</a>
			<h1 class="mt-1 flex items-center gap-2 text-2xl font-semibold text-fg">
				{data.dashboard.name}
				{#if data.dashboard.portal}<Badge tone="info">In the client portal</Badge>{/if}
			</h1>
			{#if data.dashboard.description}<p class="text-fg-muted">{data.dashboard.description}</p>{/if}
			{#if !data.dashboard.builtin && !data.dashboard.ownerUserId && audience}
				<p class="text-sm text-fg-muted">Shared with {audience}</p>
			{/if}
		</div>
		<div class="flex flex-wrap items-center gap-2">
			<div class="w-40">
				<Select
					aria-label="Range"
					value={data.range}
					options={RANGES}
					onchange={(e: Event) => pick('range', (e.currentTarget as HTMLSelectElement).value)}
				/>
			</div>
			<div class="w-56">
				<Select
					aria-label="Show numbers for"
					value={data.orgId ?? ''}
					options={[{ value: '', label: 'Everyone you can see' }, ...data.orgs]}
					onchange={(e: Event) => pick('org', (e.currentTarget as HTMLSelectElement).value)}
				/>
			</div>
			<Button variant="ghost" onclick={() => invalidateAll()} aria-label="Refresh">
				<RefreshCw size={16} />
			</Button>
			<details class="relative">
				<summary
					class="flex h-9 cursor-pointer list-none items-center gap-1 rounded-control border border-border-strong px-3 text-sm text-fg"
				>
					<Download size={16} /> Download
				</summary>
				<ul
					class="absolute right-0 z-10 mt-1 w-40 rounded-card border border-border bg-surface p-1 text-sm shadow"
					aria-label="Download as"
				>
					{#each [['pdf', 'PDF'], ['xlsx', 'Excel (XLSX)'], ['csv', 'CSV']] as [format, label] (format)}
						<li>
							<!-- eslint-disable-next-line svelte/no-navigation-without-resolve -- a file download -->
							<a
								href="{resolve('/(app)/dashboards/[id]/export', {
									id: data.dashboard.id
								})}?format={format}&{exportQuery}"
								class="block rounded px-2 py-1 text-fg hover:bg-surface-raised"
								data-sveltekit-reload
								download
							>
								{label}
							</a>
						</li>
					{/each}
				</ul>
			</details>
			{#if data.canSchedule}
				<Button onclick={() => (scheduling = true)}>
					<CalendarClock size={16} /> Email regularly
				</Button>
			{/if}
			{#if data.canSubmit && !pending}
				<Button onclick={() => (publishing = true)}><Send size={16} /> Publish</Button>
			{/if}
			{#if data.canEdit}
				<Button href={resolve('/(app)/dashboards/[id]/edit', { id: data.dashboard.id })}>
					<Pencil size={16} /> Edit
				</Button>
			{:else}
				<form method="POST" action="?/copy" use:enhance>
					<Button type="submit"><Copy size={16} /> Copy to change it</Button>
				</form>
			{/if}
		</div>
	</div>
	{#if pending}
		<div
			class="flex flex-wrap items-center justify-between gap-3 rounded-card border border-border bg-surface-raised p-3"
			role="status"
		>
			<span class="text-sm text-fg">
				Waiting for approval since {new Date(pending.submittedAt).toLocaleDateString()}. Changes you
				make now aren't part of this submission.
			</span>
			<form method="POST" action="?/withdraw" use:enhance>
				<input type="hidden" name="submission" value={pending.id} />
				<Button type="submit" variant="ghost">Withdraw</Button>
			</form>
		</div>
	{:else if latest?.status === 'rejected' && data.dashboard.ownerUserId}
		<div
			class="rounded-card border border-border bg-surface-raised p-3 text-sm text-fg"
			role="status"
		>
			<strong>Sent back</strong> by {latest.decidedByName ?? 'a reviewer'}: {latest.comment}
		</div>
	{/if}
	{#if data.dashboard.widgets.length === 0}
		<p class="text-fg-muted">No widgets yet.</p>
	{/if}
	<div class="grid grid-cols-1 gap-4 md:grid-cols-12">
		{#each data.dashboard.widgets as w (w.id)}
			<Widget
				title={w.title}
				viz={w.viz}
				width={w.width}
				answer={data.results[w.id]?.answer}
				problem={data.results[w.id]?.problem}
			/>
		{/each}
	</div>
</div>

<PublishDialog
	bind:open={publishing}
	audiences={data.audiences}
	problem={form && 'publish' in form ? (form.publish as string) : null}
/>

<ScheduleDialog
	bind:open={scheduling}
	me={data.user.id}
	teams={data.audiences.teams}
	problem={form && 'schedule' in form ? (form.schedule as string) : null}
	scheduled={form && 'scheduled' in form ? (form.scheduled as string) : null}
/>
