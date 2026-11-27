<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { INVOICE_RULE_KINDS } from '#lib/billing/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Rule = (typeof data.rules)[number];
	let ruleOpen = $state(false);
	let rule: Rule | null = $state(null);
	let ruleKind = $state<string>('edited');
	const kindLabel = (k: string) => INVOICE_RULE_KINDS.find((x) => x.value === k)?.label ?? k;
	const summary = (r: Rule) =>
		r.kind === 'total_above'
			? `Total over ${r.threshold}`
			: r.kind === 'change_above'
				? `Total moved more than ${r.threshold}% from the last`
				: r.kind === 'expression'
					? (r.expression ?? '')
					: kindLabel(r.kind);
	const said =
		(after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			toast.push({
				title: message ?? 'Done.',
				tone: result.type === 'success' ? 'success' : 'danger'
			});
			if (result.type === 'success') after?.();
		};
</script>

<svelte:head>
	<title>Billing · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	{#each data.providers as p (p.orgId)}
		<Card
			title={data.providers.length > 1 ? `Billing: ${p.name}` : 'Billing'}
			description="Time no agreement prices is billed at the default rate. Invoices are due after the payment terms."
		>
			<form
				method="POST"
				action="?/save"
				class="grid max-w-2xl gap-4 sm:grid-cols-2"
				use:enhance={() =>
					async ({ result, update }) => {
						await update({ reset: false });
						const said =
							result.type === 'success' || result.type === 'failure' ? result.data : undefined;
						const message = (said as { message?: string } | undefined)?.message;
						toast.push({
							title: message ?? 'Done.',
							tone: result.type === 'success' ? 'success' : 'danger'
						});
					}}
			>
				<input type="hidden" name="orgId" value={p.orgId} />
				<Field label="Default hourly rate" for="rate-{p.orgId}">
					<Input
						id="rate-{p.orgId}"
						name="hourlyRate"
						inputmode="decimal"
						value={p.settings?.hourlyRate ?? ''}
					/>
				</Field>
				<Field label="Currency" for="currency-{p.orgId}">
					<Input
						id="currency-{p.orgId}"
						name="currency"
						maxlength={3}
						value={p.settings?.currency ?? 'USD'}
					/>
				</Field>
				<Field label="Payment terms (days)" for="days-{p.orgId}">
					<Input
						id="days-{p.orgId}"
						name="paymentDays"
						type="number"
						min={0}
						max={365}
						value={p.settings?.paymentDays ?? 30}
					/>
				</Field>
				<label class="flex items-end gap-2 pb-2 text-sm text-fg">
					<input
						type="checkbox"
						name="approvedTimeOnly"
						checked={p.settings?.approvedTimeOnly ?? false}
					/>
					Bill only time on approved timesheets
				</label>
				<div class="sm:col-span-2"><Button type="submit" variant="primary">Save</Button></div>
			</form>
		</Card>
	{/each}

	<Card
		title="Invoice review"
		description="Which invoices need an internal review before they can be approved, and who reviews them. The Invoice review pack (in Packs) runs the review; an approved review approves the invoice."
	>
		{#snippet actions()}
			{#if data.canRules}
				<Button
					onclick={() => {
						rule = null;
						ruleKind = 'total_above';
						ruleOpen = true;
					}}
				>
					<Plus size={16} /> New rule
				</Button>
			{/if}
		{/snippet}
		{#if data.rules.length === 0}
			<p class="text-sm text-fg-muted">No rules: no invoice needs review.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Invoice review rules">
				{#each data.rules as r (r.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-semibold text-fg">
								{r.name}
								{#if !r.active}<Badge>Off</Badge>{/if}
							</p>
							<p class="truncate text-xs text-fg-muted">
								{summary(r)} · reviewed by <span class="font-mono">{r.reviewers}</span>
							</p>
						</div>
						{#if data.canRules}
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
								<form method="POST" action="?/deleteRule" use:enhance={said()}>
									<input type="hidden" name="id" value={r.id} />
									<Button type="submit" size="sm" variant="ghost" aria-label="Remove {r.name}">
										Remove
									</Button>
								</form>
							</div>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open={ruleOpen} title={rule ? `Edit ${rule.name}` : 'New invoice review rule'}>
	<form
		id="rule-form"
		method="POST"
		action="?/saveRule"
		class="flex flex-col gap-4"
		use:enhance={said(() => (ruleOpen = false))}
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
				options={INVOICE_RULE_KINDS.map((k) => ({ value: k.value, label: k.label }))}
			/>
		</Field>
		{#if ruleKind === 'total_above' || ruleKind === 'change_above'}
			<Field label={ruleKind === 'total_above' ? 'Amount' : 'Percent'} for="rule-threshold">
				<Input
					id="rule-threshold"
					name="threshold"
					inputmode="decimal"
					required
					value={rule?.threshold ?? ''}
				/>
			</Field>
		{:else if ruleKind === 'expression'}
			<Field
				label="Expression"
				for="rule-expression"
				hint="Over invoice: e.g. invoice.total | float > 10000 and invoice.currency == 'USD'"
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
