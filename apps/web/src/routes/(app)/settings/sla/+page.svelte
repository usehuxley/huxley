<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, toast } from '@usehuxley/ui';
	import HoursFields from '#lib/crm/HoursFields.svelte';
	import { formatHolidays, type Holiday } from '#lib/tickets/holidays.ts';
	import { hoursSummary } from '#lib/crm/labels.ts';
	import { formatDuration } from '#lib/tickets/duration.ts';
	import { PRIORITIES } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	type Policy = (typeof data.policies)[number];
	let editing: Policy | null = $state(null);
	let open = $state(false);
	let source = $state('client_site');
	let errors: Record<string, string> = $derived(form?.errors ?? {});

	const SOURCES = [
		{
			value: 'client_site',
			label: "The client's site hours",
			hint: "Opening hours at the client's primary site; the hours below where it has none."
		},
		{ value: 'policy', label: 'These hours', hint: 'The hours below, in the time zone below.' },
		{ value: 'always', label: 'Around the clock', hint: 'Every hour of every day counts.' }
	] as const;

	const zones = (() => {
		try {
			return Intl.supportedValuesOf('timeZone');
		} catch {
			return [];
		}
	})();

	function show(policy: Policy | null) {
		editing = policy;
		source = policy?.hoursSource ?? 'client_site';
		open = true;
	}

	function holidaysOf(policy: Policy): string {
		const n = policy.holidays.length;
		return n === 0 ? '' : ` · ${n} holiday${n === 1 ? '' : 's'}`;
	}

	function hoursOf(policy: Policy): string {
		switch (policy.hoursSource) {
			case 'always':
				return 'Around the clock';
			case 'policy':
				return `${hoursSummary(policy.hours)}${policy.timezone ? ` (${policy.timezone})` : ''}${holidaysOf(policy)}`;
			default:
				return `The client's site hours${holidaysOf(policy)}`;
		}
	}

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>SLA policies · Settings · Huxley</title>
</svelte:head>

<Card
	title="SLA policies"
	description="Targets for the first response and for resolution, by priority, counted in business hours. Choose a policy for an organization on its settings page; the default applies everywhere else."
>
	{#snippet actions()}
		<Button onclick={() => show(null)}><Plus size={16} /> New policy</Button>
	{/snippet}
	<ul class="flex flex-col divide-y divide-border">
		{#each data.policies as policy (policy.id)}
			<li class="flex flex-col gap-3 py-4 first:pt-0 last:pb-0">
				<div class="flex flex-wrap items-start justify-between gap-3">
					<div class="min-w-0">
						<p class="flex items-center gap-2 text-sm font-medium text-fg">
							{policy.name}
							{#if policy.isDefault}<Badge tone="accent">Default</Badge>{/if}
						</p>
						<p class="mt-0.5 text-sm text-fg-muted">{hoursOf(policy)}</p>
						{#if policy.description}
							<p class="mt-0.5 text-sm text-fg-muted">{policy.description}</p>
						{/if}
					</div>
					<div class="flex gap-2">
						<Button size="sm" onclick={() => show(policy)}>Edit</Button>
						{#if !policy.isDefault}
							<form method="POST" action="?/archive" use:enhance={done('Policy archived.')}>
								<input type="hidden" name="id" value={policy.id} />
								<Button size="sm" type="submit" variant="ghost">Archive</Button>
							</form>
						{/if}
					</div>
				</div>
				<table class="w-full max-w-md text-left text-sm" aria-label="{policy.name} targets">
					<thead class="text-xs text-fg-muted">
						<tr>
							<th scope="col" class="py-1 font-medium">Priority</th>
							<th scope="col" class="py-1 font-medium">First response</th>
							<th scope="col" class="py-1 font-medium">Resolution</th>
						</tr>
					</thead>
					<tbody>
						{#each PRIORITIES as p (p.value)}
							{@const target = policy.targets[p.value]}
							<tr>
								<td class="py-1"><Badge tone={p.tone}>{p.short}</Badge></td>
								<td class="py-1 tabular-nums">{formatDuration(target?.response) || '—'}</td>
								<td class="py-1 tabular-nums">{formatDuration(target?.resolution) || '—'}</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</li>
		{/each}
	</ul>
</Card>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New SLA policy'}>
	<form
		method="POST"
		action="?/save"
		class="flex flex-col gap-4"
		use:enhance={done('Policy saved.', () => (open = false))}
	>
		{#if form?.message && !form?.saved}
			<p role="alert" class="text-sm text-danger">{form.message}</p>
		{/if}
		{#if editing}
			<input type="hidden" name="id" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
		{/if}
		<Field label="Name" for="sla-name" error={errors.name}>
			<Input id="sla-name" name="name" required maxlength={100} value={editing?.name ?? ''} />
		</Field>
		<Field label="Description" for="sla-description" error={errors.description}>
			<Input
				id="sla-description"
				name="description"
				maxlength={2000}
				value={editing?.description ?? ''}
			/>
		</Field>

		<fieldset class="flex flex-col gap-2">
			<legend class="text-sm font-medium text-fg">Targets</legend>
			<p class="text-sm text-fg-muted">
				In business time, like 30m, 4h or 1h 30m. Leave one empty for no target.
			</p>
			{#if errors.targets}<p class="text-sm text-danger">{errors.targets}</p>{/if}
			<div class="grid grid-cols-[4rem_1fr_1fr] items-center gap-2 text-sm">
				<span></span>
				<span class="text-xs text-fg-muted">First response</span>
				<span class="text-xs text-fg-muted">Resolution</span>
				{#each PRIORITIES as p (p.value)}
					{@const target = editing?.targets[p.value]}
					<span><Badge tone={p.tone}>{p.short}</Badge></span>
					<Input
						name="response_{p.value}"
						aria-label="{p.short} first response"
						value={formatDuration(target?.response)}
						invalid={!!errors[`response_${p.value}`]}
					/>
					<Input
						name="resolution_{p.value}"
						aria-label="{p.short} resolution"
						value={formatDuration(target?.resolution)}
						invalid={!!errors[`resolution_${p.value}`]}
					/>
				{/each}
			</div>
		</fieldset>

		<fieldset class="flex flex-col gap-2">
			<legend class="text-sm font-medium text-fg">Business hours</legend>
			{#each SOURCES as option (option.value)}
				<label class="flex items-start gap-2 text-sm text-fg">
					<input
						type="radio"
						name="hoursSource"
						value={option.value}
						bind:group={source}
						class="mt-1"
					/>
					<span>
						{option.label}
						<span class="block text-fg-muted">{option.hint}</span>
					</span>
				</label>
			{/each}
		</fieldset>
		{#if source !== 'always'}
			<HoursFields
				hours={editing?.hours ?? null}
				label={source === 'policy' ? 'Hours' : 'Hours where the client has none'}
				error={errors.hours}
			/>
			<Field
				label="Time zone"
				for="sla-tz"
				hint="For these hours. Empty: the organization's time zone setting."
				error={errors.timezone}
			>
				<Input id="sla-tz" name="timezone" value={editing?.timezone ?? ''} list="sla-zones" />
				<datalist id="sla-zones">
					{#each zones as zone (zone)}<option value={zone}></option>{/each}
				</datalist>
			</Field>
			<Field
				label="Holidays"
				for="sla-holidays"
				hint="Days the clocks don't count: one a line, a date then a name, e.g. 2026-12-25 Christmas Day."
				error={errors.holidays}
			>
				<textarea
					id="sla-holidays"
					name="holidays"
					rows="5"
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 font-mono text-sm text-fg"
					value={formatHolidays((editing?.holidays ?? []) as Holiday[])}></textarea>
			</Field>
		{/if}

		<label class="flex items-center gap-2 text-sm text-fg">
			<input
				type="checkbox"
				name="isDefault"
				checked={editing?.isDefault ?? false}
				disabled={editing?.isDefault}
			/>
			The default policy
		</label>
		{#if editing?.isDefault}
			<!-- Disabled checkboxes aren't posted: keep it the default. -->
			<input type="hidden" name="isDefault" value="on" />
		{/if}

		<div class="flex justify-end gap-2">
			<Button onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save policy</Button>
		</div>
	</form>
</Dialog>
