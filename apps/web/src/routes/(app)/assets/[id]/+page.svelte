<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Pencil, X } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import { KINDS, relationLabel, statusLabel, statusTone } from '#lib/assets/form.ts';
	import DependencyMap from '#lib/assets/DependencyMap.svelte';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let a = $derived(data.asset);

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: true });
			const message =
				(result.type === 'success' || result.type === 'failure'
					? (result.data as { message?: string } | undefined)?.message
					: undefined) ?? 'Done.';
			toast.push({ title: message, tone: result.type === 'success' ? 'success' : 'danger' });
		};
	const WARRANTY_KINDS: Record<string, string> = {
		manufacturer: 'Manufacturer',
		extended: 'Extended',
		support: 'Support contract',
		other: 'Other'
	};
	/** Days from today to a YYYY-MM-DD date (negative: past). */
	const daysLeft = (date: string) =>
		Math.round((Date.parse(date) - Date.parse(new Date().toISOString().slice(0, 10))) / 86_400_000);
	const show = (v: unknown): string =>
		v === true
			? 'Yes'
			: v === false
				? 'No'
				: Array.isArray(v)
					? v.map(show).join(', ')
					: typeof v === 'string' || typeof v === 'number'
						? String(v)
						: '';
	const optionLabel = (key: string, value: unknown): string => {
		const field = data.type?.fields.find((f) => f.key === key);
		const options = field?.options ?? [];
		const one = (v: unknown) => options.find((o) => o.value === v)?.label ?? show(v);
		return Array.isArray(value) ? value.map(one).join(', ') : one(value);
	};
</script>

{#snippet from(key: string)}
	{@const source = key ? a.fieldSources[key] : undefined}
	{#if source && source !== 'manual'}
		<span
			class="ml-1 rounded bg-surface-sunken px-1 text-[0.65rem] text-fg-subtle"
			title="Kept in step with {source}"
		>
			{source}
		</span>
	{/if}
{/snippet}

<svelte:head>
	<title>{a.name} · Assets · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-5xl flex-col gap-6">
	<header class="flex flex-wrap items-start justify-between gap-4">
		<div class="min-w-0">
			<p class="text-sm">
				<a class="text-fg-muted hover:text-fg" href={resolve('/(app)/assets')}>Assets</a>
			</p>
			<h1 class="text-2xl font-semibold break-words text-fg">{a.name}</h1>
			<div class="mt-2 flex flex-wrap items-center gap-2 text-sm text-fg-muted">
				<Badge tone={statusTone(a.status)}>{statusLabel(a.status)}</Badge>
				<span>{a.typeName}</span>
				<span>
					· <a class="hover:underline" href={resolve('/(app)/clients/[id]', { id: a.orgId })}>
						{a.orgName}
					</a>
				</span>
			</div>
		</div>
		{#if a.canWrite}
			<div class="flex gap-2">
				<Button href={resolve('/(app)/assets/[id]/edit', { id: a.id })} variant="secondary">
					<Pencil size={16} /> Edit
				</Button>
				<form method="POST" action="?/delete" use:enhance={done}>
					<Button type="submit" variant="ghost">Delete</Button>
				</form>
			</div>
		{/if}
	</header>

	<div class="grid gap-6 lg:grid-cols-[1fr_20rem]">
		<div class="flex flex-col gap-6">
			<Card title="Details">
				<dl class="grid gap-x-6 gap-y-3 text-sm sm:grid-cols-2" aria-label="Details">
					{#each [['Serial number', a.serialNumber, 'serial_number'], ['Hostname', a.hostname, 'hostname'], ['Manufacturer', a.manufacturer, 'manufacturer'], ['Model', a.model, 'model'], ['Site', a.siteName ?? '', ''], ['Used by', a.contactName ?? '', ''], ['Purchased on', a.purchasedOn ?? '', ''], ['Warranty ends on', a.warrantyEndsOn ?? '', '']] as [label, value, key] (label)}
						{#if value}
							<div>
								<dt class="text-fg-muted">{label}{@render from(key)}</dt>
								<dd class="text-fg">{value}</dd>
							</div>
						{/if}
					{/each}
					{#each data.type?.fields ?? [] as field (field.key)}
						{#if a.attributes[field.key] !== undefined}
							<div>
								<dt class="text-fg-muted">
									{field.label}{@render from(`attributes.${field.key}`)}
								</dt>
								<dd class="text-fg">{optionLabel(field.key, a.attributes[field.key])}</dd>
							</div>
						{/if}
					{/each}
				</dl>
				{#if a.notes}<p class="mt-4 text-sm whitespace-pre-wrap text-fg">{a.notes}</p>{/if}
				{#if Object.keys(a.externalIds).length > 0 || a.syncedAt}
					<p class="mt-4 text-xs text-fg-subtle" data-testid="asset-sync">
						{#each Object.entries(a.externalIds) as [source, id] (source)}
							<span class="mr-3">{source}: <span class="font-mono">{String(id)}</span></span>
						{/each}
						{#if a.syncedAt}Last synced {new Date(a.syncedAt).toLocaleString()}{/if}
					</p>
				{/if}
			</Card>

			{#if data.map && data.map.edges.length > 0}
				<Card
					title="Dependency map"
					description="Click an asset to open it. Arrows point at what's depended on."
				>
					<DependencyMap map={data.map} centre={a.id} />
					{#if data.map.truncated}<p class="mt-2 text-xs text-fg-muted">
							Showing the nearest 150 assets.
						</p>{/if}
				</Card>
			{/if}
			<Card title="Relationships">
				{#if a.relationships.length === 0}
					<p class="text-sm text-fg-muted">None yet.</p>
				{:else}
					<ul class="flex flex-col gap-2 text-sm" aria-label="Relationships">
						{#each a.relationships as r (r.id + r.direction)}
							<li class="flex items-center justify-between gap-2">
								<span>
									<span class="text-fg-muted">{a.name} {relationLabel(r.kind, r.direction)}</span>
									<a
										class="text-fg hover:underline"
										href={resolve('/(app)/assets/[id]', { id: r.otherId })}
									>
										{r.otherName}
									</a>
									<span class="text-xs text-fg-subtle">{r.otherType}</span>
								</span>
								{#if a.canWrite}
									<form method="POST" action="?/unrelate" use:enhance={done}>
										<input type="hidden" name="relationshipId" value={r.id} />
										<button
											type="submit"
											class="text-fg-subtle hover:text-fg"
											aria-label="Remove: {relationLabel(r.kind, r.direction)} {r.otherName}"
										>
											<X size={14} />
										</button>
									</form>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				{#if a.canWrite && data.candidates.length > 0}
					<form
						method="POST"
						action="?/relate"
						use:enhance={done}
						class="mt-4 flex flex-wrap items-end gap-2"
					>
						<span class="pb-2 text-sm text-fg-muted">{a.name}</span>
						<div class="w-40">
							<Field label="Relationship" for="rel-kind">
								<Select
									id="rel-kind"
									name="kind"
									options={KINDS.map((k) => ({ value: k.value, label: k.label }))}
								/>
							</Field>
						</div>
						<div class="w-64">
							<Field label="Asset" for="rel-to">
								<Select id="rel-to" name="toId" options={data.candidates} />
							</Field>
						</div>
						<Button type="submit">Add</Button>
					</form>
				{/if}
			</Card>
		</div>

		<div class="flex flex-col gap-6">
			<Card
				title="If it goes down"
				description="What runs on it, depends on it, or is hosted or licensed by it."
			>
				{#if data.impact.length === 0}
					<p class="text-sm text-fg-muted">Nothing depends on it.</p>
				{:else}
					<ul class="flex flex-col gap-2 text-sm" aria-label="Impact">
						{#each data.impact as hit (hit.id)}
							<li style="padding-left: {(hit.depth - 1) * 0.75}rem">
								<a
									class="text-fg hover:underline"
									href={resolve('/(app)/assets/[id]', { id: hit.id })}
								>
									{hit.name}
								</a>
								<p class="text-xs text-fg-subtle">
									{hit.typeName} · {relationLabel(
										hit.viaKind,
										hit.viaKind === 'hosts' ? 'in' : 'out'
									)}
									{hit.viaName}
								</p>
							</li>
						{/each}
					</ul>
				{/if}
			</Card>
			<Card title="Warranties">
				{#if data.warranties.length === 0}
					<p class="text-sm text-fg-muted">None recorded.</p>
				{:else}
					<ul class="flex flex-col gap-2 text-sm" aria-label="Warranties">
						{#each data.warranties as w (w.id)}
							{@const left = daysLeft(w.endsOn)}
							<li class="flex items-start justify-between gap-2">
								<div class="min-w-0">
									<p class="text-fg">
										{WARRANTY_KINDS[w.kind] ?? w.kind}{w.provider ? ` · ${w.provider}` : ''}
										{#if w.source !== 'manual'}<span
												class="ml-1 rounded bg-surface-sunken px-1 text-[0.65rem] text-fg-subtle"
											>
												{w.source}
											</span>{/if}
									</p>
									<p
										class="text-xs {left < 0
											? 'text-danger'
											: left <= 60
												? 'text-warning'
												: 'text-fg-subtle'}"
									>
										{left < 0 ? 'Ended' : 'Ends'}
										{w.endsOn}{left >= 0 && left <= 60 ? ` (in ${left} days)` : ''}{w.reference
											? ` · ${w.reference}`
											: ''}
									</p>
								</div>
								{#if a.canWrite}
									<form method="POST" action="?/removeWarranty" use:enhance={done}>
										<input type="hidden" name="warrantyId" value={w.id} />
										<button
											type="submit"
											class="text-fg-subtle hover:text-fg"
											aria-label="Remove the warranty ending {w.endsOn}"
										>
											<X size={14} />
										</button>
									</form>
								{/if}
							</li>
						{/each}
					</ul>
				{/if}
				{#if a.canWrite}
					<form
						method="POST"
						action="?/addWarranty"
						use:enhance={done}
						class="mt-4 grid gap-2 text-sm"
					>
						<Field label="Kind" for="w-kind">
							<Select
								id="w-kind"
								name="kind"
								options={Object.entries(WARRANTY_KINDS).map(([value, label]) => ({ value, label }))}
							/>
						</Field>
						<Field label="Provider" for="w-provider">
							<Input id="w-provider" name="provider" maxlength={200} />
						</Field>
						<Field label="Reference" for="w-reference" hint="Contract number or service tag">
							<Input id="w-reference" name="reference" maxlength={200} />
						</Field>
						<Field label="Starts" for="w-starts">
							<Input id="w-starts" name="startsOn" type="date" />
						</Field>
						<Field label="Ends" for="w-ends">
							<Input id="w-ends" name="endsOn" type="date" required />
						</Field>
						<div><Button type="submit" size="sm">Add warranty</Button></div>
					</form>
				{/if}
			</Card>
			{#if data.tickets.length > 0}
				<Card title="Tickets">
					<ul class="flex flex-col gap-2 text-sm" aria-label="Asset tickets">
						{#each data.tickets as t (t.id)}
							<li>
								<a
									class="text-fg hover:underline"
									href={resolve('/(app)/tickets/[id]', { id: t.id })}
								>
									<span class="font-mono text-xs text-fg-muted">{t.reference}</span>
									{t.title}
								</a>
								<p class="text-xs text-fg-subtle">{t.statusName}</p>
							</li>
						{/each}
					</ul>
				</Card>
			{/if}
		</div>
	</div>
</div>
