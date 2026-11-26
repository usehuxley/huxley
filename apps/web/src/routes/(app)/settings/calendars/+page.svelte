<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Badge, Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	const SOURCES = [
		{ value: 'provider', label: 'Microsoft 365' },
		{ value: 'ics', label: 'iCalendar feed' },
		{ value: 'none', label: 'Not synced' }
	];
	const when = (v: string) => new Date(v).toLocaleString();
	// The source picked in each row, before it's saved (shows the right field).
	let picked = $state<Record<string, string>>({});
	const sourceOf = (p: { userId: string; source: string }) => picked[p.userId] ?? p.source;
	let current = $derived(data.settings ? `${data.settings.orgId}|${data.settings.connection}` : '');
</script>

<svelte:head><title>Calendars · Settings · Huxley</title></svelte:head>

<div class="flex flex-col gap-4">
	<Card
		title="Calendars"
		description="People's busy time from their work calendars, so dispatch, the dispatch board and capacity planning know when they're actually free. Only free/busy is read unless you keep subjects."
	>
		<div class="flex flex-col gap-3 text-sm">
			{#if form?.message}<p class="text-danger" role="alert">{form.message}</p>{/if}
			{#if form?.saved}<p class="text-fg" role="status">{form.saved}</p>{/if}
			<p class="text-fg-muted">
				Install the <a href={resolve('/(app)/settings/packs')} class="text-accent hover:underline">
					Calendar sync
				</a> pack to read calendars every five minutes.
			</p>
			<form method="POST" action="?/settings" class="flex flex-col gap-3" use:enhance>
				<Field label="Microsoft 365 connection" for="cal-connection">
					<Select
						id="cal-connection"
						name="connection"
						value={current}
						options={[
							{
								value: '',
								label: data.connections.length
									? 'Choose a connection'
									: 'No Microsoft Graph connections yet'
							},
							...data.connections
						]}
					/>
				</Field>
				<p class="text-xs text-fg-muted">
					The connection's app needs Calendars.Read (or Calendars.ReadBasic.All). People are read at
					their email, or at the address you set below.
				</p>
				<label class="flex items-center gap-2 text-fg">
					<input
						type="checkbox"
						name="keepSubjects"
						checked={data.settings?.keepSubjects ?? false}
					/>
					Keep event subjects (never private ones)
				</label>
				<label class="flex items-center gap-2 text-fg">
					<input type="checkbox" name="writeBack" checked={data.settings?.writeBack ?? false} />
					Put booked appointments in people's calendars (needs Calendars.ReadWrite)
				</label>
				<div class="flex gap-2">
					<Button type="submit" variant="primary">Save</Button>
					{#if data.settings}
						<Button type="submit" variant="ghost" formaction="?/clear">
							Stop reading Microsoft 365
						</Button>
					{/if}
				</div>
			</form>
		</div>
	</Card>

	<Card title="People" description="Whose calendar is read, and from where.">
		<ul class="flex flex-col divide-y divide-border" aria-label="People's calendars">
			{#each data.people as p (p.userId)}
				<li class="py-3">
					<form
						method="POST"
						action="?/person"
						class="grid gap-2 md:grid-cols-[14rem_10rem_1fr_auto] md:items-end"
						use:enhance
					>
						<input type="hidden" name="userId" value={p.userId} />
						<div class="min-w-0">
							<p class="truncate font-medium text-fg">{p.name}</p>
							<p class="truncate text-xs text-fg-muted">{p.email}</p>
						</div>
						<Field label="Source" for="src-{p.userId}">
							<Select
								id="src-{p.userId}"
								name="source"
								value={sourceOf(p)}
								options={SOURCES}
								onchange={(e: Event) =>
									(picked[p.userId] = (e.currentTarget as HTMLSelectElement).value)}
							/>
						</Field>
						{#if sourceOf(p) === 'ics'}
							<Field label="Feed URL" for="ics-{p.userId}">
								<Input
									id="ics-{p.userId}"
									name="icsUrl"
									type="url"
									placeholder={p.hasFeed
										? 'Set (enter a new one to replace it)'
										: 'https://…/basic.ics'}
								/>
							</Field>
						{:else}
							<Field label="Calendar address" for="cal-{p.userId}">
								<Input
									id="cal-{p.userId}"
									name="calendar"
									value={p.calendar ?? ''}
									placeholder={p.email}
								/>
							</Field>
						{/if}
						<Button type="submit" size="sm">Save</Button>
						<p class="text-xs md:col-span-4">
							{#if p.lastError}
								<Badge tone="danger">Not synced</Badge>
								<span class="text-fg-muted">{p.lastError}</span>
							{:else if p.lastSyncedAt}
								<span class="text-fg-muted">Synced {when(p.lastSyncedAt)}</span>
							{:else}
								<span class="text-fg-muted">Not synced yet</span>
							{/if}
						</p>
					</form>
				</li>
			{/each}
		</ul>
	</Card>
</div>
