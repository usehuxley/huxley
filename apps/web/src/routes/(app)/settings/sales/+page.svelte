<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { ArrowDown, ArrowUp, Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { RULE_KINDS, label } from '#lib/sales/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Rule = (typeof data.rules)[number];
	let ruleOpen = $state(false);
	let rule: Rule | null = $state(null);
	let ruleKind = $state<string>('always');
	const ruleSummary = (r: Rule) => {
		switch (r.kind) {
			case 'margin_below':
				return `Margin under ${r.threshold}%`;
			case 'first_year_above':
				return `First year over ${r.threshold}`;
			case 'discount_above':
				return `A line more than ${r.threshold}% off`;
			case 'category':
				return `Includes ${r.categories.join(', ')}`;
			case 'expression':
				return r.expression ?? '';
			default:
				return label(RULE_KINDS, r.kind);
		}
	};

	type Stage = (typeof data.stages)[number];
	let open = $state(false);
	let editing: Stage | null = $state(null);
	let kind = $state('open');

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
	const KINDS = [
		{ value: 'open', label: 'Open (worked)' },
		{ value: 'won', label: 'Won' },
		{ value: 'lost', label: 'Lost' }
	];
</script>

<svelte:head>
	<title>Sales pipeline · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Stages"
		description="What an opportunity moves through, in order, with the chance of winning at each. Won and lost stages close it."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					editing = null;
					kind = 'open';
					open = true;
				}}
			>
				<Plus size={16} /> New stage
			</Button>
		{/snippet}
		<ol class="flex flex-col divide-y divide-border" aria-label="Stages">
			{#each data.stages as s, i (s.id)}
				<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
					<div class="flex min-w-0 items-center gap-2">
						<span class="w-6 font-mono text-xs text-fg-subtle">{i + 1}</span>
						<span class="text-sm font-semibold text-fg">{s.name}</span>
						<Badge tone={s.kind === 'won' ? 'success' : s.kind === 'lost' ? 'danger' : 'info'}>
							{s.kind === 'open' ? `${s.probability}%` : s.kind === 'won' ? 'Won' : 'Lost'}
						</Badge>
					</div>
					<div class="flex shrink-0 gap-1">
						<form method="POST" action="?/reorder" use:enhance={done('Moved.')} class="flex gap-1">
							{#each data.stages as x (x.id)}<input type="hidden" name="ids" value={x.id} />{/each}
							<input type="hidden" name="id" value={s.id} />
							<Button
								type="submit"
								name="dir"
								value="up"
								size="sm"
								variant="ghost"
								disabled={i === 0}
								aria-label="Move {s.name} up"
							>
								<ArrowUp size={14} />
							</Button>
							<Button
								type="submit"
								name="dir"
								value="down"
								size="sm"
								variant="ghost"
								disabled={i === data.stages.length - 1}
								aria-label="Move {s.name} down"
							>
								<ArrowDown size={14} />
							</Button>
						</form>
						<Button
							size="sm"
							onclick={() => {
								editing = s;
								kind = s.kind;
								open = true;
							}}
						>
							Edit
						</Button>
						<form method="POST" action="?/archiveStage" use:enhance={done('Removed.')}>
							<input type="hidden" name="id" value={s.id} />
							<Button type="submit" size="sm" variant="ghost" aria-label="Remove {s.name}">
								Remove
							</Button>
						</form>
					</div>
				</li>
			{/each}
		</ol>
	</Card>

	<Card
		title="Quote review"
		description="When a quote needs an internal review before it's sent, and who reviews it. The Quote review pack (in Packs) runs the review; without a matching rule, quotes go straight out."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					rule = null;
					ruleKind = 'margin_below';
					ruleOpen = true;
				}}
			>
				<Plus size={16} /> New rule
			</Button>
		{/snippet}
		{#if data.rules.length === 0}
			<p class="text-sm text-fg-muted">No rules: no quote needs review.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Review rules">
				{#each data.rules as r (r.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-semibold text-fg">
								{r.name}
								{#if !r.active}<Badge>Off</Badge>{/if}
							</p>
							<p class="truncate text-xs text-fg-muted">
								{ruleSummary(r)} · reviewed by
								<span class="font-mono">{r.reviewers}</span>
							</p>
						</div>
						<div class="flex shrink-0 gap-1">
							<Button
								size="sm"
								onclick={() => {
									rule = r;
									ruleKind = r.kind;
									ruleOpen = true;
								}}
							>
								Edit
							</Button>
							<form method="POST" action="?/deleteRule" use:enhance={done('Removed.')}>
								<input type="hidden" name="id" value={r.id} />
								<Button type="submit" size="sm" variant="ghost" aria-label="Remove {r.name}">
									Remove
								</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<div class="grid gap-6 md:grid-cols-2">
		{#each ['won', 'lost'] as k (k)}
			<Card title={k === 'won' ? 'Why deals are won' : 'Why deals are lost'}>
				<ul
					class="flex flex-col divide-y divide-border"
					aria-label={k === 'won' ? 'Win reasons' : 'Loss reasons'}
				>
					{#each data.reasons.filter((r) => r.kind === k) as r (r.id)}
						<li class="flex items-center justify-between gap-3 py-1.5 first:pt-0">
							<span class="text-sm text-fg">{r.name}</span>
							<form method="POST" action="?/archiveReason" use:enhance={done('Removed.')}>
								<input type="hidden" name="id" value={r.id} />
								<Button type="submit" size="sm" variant="ghost" aria-label="Remove {r.name}">
									Remove
								</Button>
							</form>
						</li>
					{/each}
				</ul>
				<form
					method="POST"
					action="?/addReason"
					class="mt-3 flex gap-2"
					use:enhance={done('Added.')}
				>
					<input type="hidden" name="kind" value={k} />
					<label for="reason-{k}" class="sr-only">New reason</label>
					<Input
						id="reason-{k}"
						name="name"
						required
						maxlength={100}
						placeholder="A new reason"
						class="flex-1"
					/>
					<Button type="submit">Add</Button>
				</form>
			</Card>
		{/each}
	</div>
</div>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'New stage'}>
	<form
		id="stage-form"
		method="POST"
		action="?/saveStage"
		class="flex flex-col gap-4"
		use:enhance={done('Saved.', () => (open = false))}
	>
		{#if editing}<input type="hidden" name="id" value={editing.id} />{/if}
		<Field label="Name" for="stage-name">
			<Input id="stage-name" name="name" required maxlength={60} value={editing?.name ?? ''} />
		</Field>
		<Field label="Kind" for="stage-kind">
			<Select id="stage-kind" name="kind" bind:value={kind} options={KINDS} />
		</Field>
		{#if kind === 'open'}
			<Field label="Chance of winning %" for="stage-probability">
				<Input
					id="stage-probability"
					name="probability"
					type="number"
					min={0}
					max={100}
					required
					value={editing?.probability ?? 50}
				/>
			</Field>
		{/if}
	</form>
	{#snippet footer()}
		<Button onclick={() => (open = false)}>Cancel</Button>
		<Button type="submit" form="stage-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>

<Dialog bind:open={ruleOpen} title={rule ? `Edit ${rule.name}` : 'New review rule'}>
	<form
		id="rule-form"
		method="POST"
		action="?/saveRule"
		class="flex flex-col gap-4"
		use:enhance={done('Saved.', () => (ruleOpen = false))}
	>
		{#if rule}<input type="hidden" name="id" value={rule.id} />{/if}
		<Field label="Name" for="rule-name">
			<Input id="rule-name" name="name" required maxlength={100} value={rule?.name ?? ''} />
		</Field>
		<Field label="Needs review when" for="rule-kind">
			<Select
				id="rule-kind"
				name="kind"
				bind:value={ruleKind}
				options={RULE_KINDS.map((k) => ({ value: k.value, label: k.label }))}
			/>
		</Field>
		{#if ruleKind === 'margin_below' || ruleKind === 'first_year_above' || ruleKind === 'discount_above'}
			<Field label={ruleKind === 'first_year_above' ? 'Amount' : 'Percent'} for="rule-threshold">
				<Input
					id="rule-threshold"
					name="threshold"
					inputmode="decimal"
					required
					value={rule?.threshold ?? ''}
				/>
			</Field>
		{:else if ruleKind === 'category'}
			<Field label="Categories" for="rule-categories" hint="Separated by commas">
				<Input
					id="rule-categories"
					name="categories"
					required
					value={rule?.categories.join(', ') ?? ''}
				/>
			</Field>
		{:else if ruleKind === 'expression'}
			<Field
				label="Expression"
				for="rule-expression"
				hint="Over quote: e.g. quote.totals.firstYear | float > 25000 and quote.currency == 'USD'"
			>
				<Input
					id="rule-expression"
					name="expression"
					required
					maxlength={2000}
					class="font-mono"
					value={rule?.expression ?? ''}
				/>
			</Field>
		{/if}
		<Field
			label="Reviewed by"
			for="rule-reviewers"
			hint="role:<key>, team:<id> or user:<id>, separated by commas; the first decision counts"
		>
			<Input
				id="rule-reviewers"
				name="reviewers"
				required
				maxlength={500}
				class="font-mono"
				value={rule?.reviewers ?? 'role:admin'}
			/>
		</Field>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="active" checked={rule?.active ?? true} /> On
		</label>
	</form>
	{#snippet footer()}
		<Button onclick={() => (ruleOpen = false)}>Cancel</Button>
		<Button type="submit" form="rule-form" variant="primary">Save</Button>
	{/snippet}
</Dialog>
