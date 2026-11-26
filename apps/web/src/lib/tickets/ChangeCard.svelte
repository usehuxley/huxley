<!-- A change ticket's change: type, risk, plans, window, assets, impact, and its way through approval. -->
<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Badge, Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Change = components['schemas']['ChangeRecord'];
	let {
		change,
		questions,
		impact,
		assets,
		canWrite
	}: {
		change: Change;
		questions: components['schemas']['RiskQuestion'][];
		impact: components['schemas']['ChangeImpact'] | null;
		assets: { id: string; name: string }[];
		canWrite: boolean;
	} = $props();

	let editable = $derived(canWrite && (change.state === 'draft' || change.state === 'rejected'));
	const answers = $derived((change.riskAnswers ?? {}) as Record<string, string | undefined>);
	const STATE: Record<string, [string, 'neutral' | 'info' | 'success' | 'danger' | 'warning']> = {
		draft: ['Draft', 'neutral'],
		submitted: ['Waiting for approval', 'info'],
		approved: ['Approved', 'success'],
		rejected: ['Rejected', 'danger'],
		implemented: ['Implemented', 'success'],
		reviewed: ['Reviewed', 'success'],
		cancelled: ['Cancelled', 'neutral']
	};
	/** An RFC 3339 time as a datetime-local value. */
	const local = (t: string | null | undefined) => {
		if (!t) return '';
		const d = new Date(t);
		const pad = (n: number) => String(n).padStart(2, '0');
		return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
	};
	const when = (t: string | null | undefined) => (t ? new Date(t).toLocaleString() : '–');
	const done =
		(message: string): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				const data = result.data as { change?: { conflicts?: string[] } } | undefined;
				const conflicts = data?.change?.conflicts ?? [];
				toast.push({
					title: conflicts.length ? `${message} It overlaps ${conflicts.join(', ')}.` : message,
					tone: conflicts.length ? 'warning' : 'success'
				});
			} else if (result.type === 'failure') {
				const data = result.data as { change?: { message?: string } } | undefined;
				toast.push({ title: data?.change?.message ?? 'That didn’t work.', tone: 'danger' });
			}
		};
</script>

<Card title="Change">
	<div class="flex flex-wrap items-center gap-2">
		<Badge tone={STATE[change.state]?.[1] ?? 'neutral'}>
			{STATE[change.state]?.[0] ?? change.state}
		</Badge>
		{#if change.riskLevel}
			<Badge
				tone={change.riskLevel === 'high'
					? 'danger'
					: change.riskLevel === 'medium'
						? 'warning'
						: 'neutral'}
			>
				{change.riskLevel} risk ({change.riskScore})
			</Badge>
		{/if}
		<span class="text-sm text-fg-muted">{change.changeType} change</span>
	</div>
	{#if change.decidedBy}
		<p class="mt-2 text-sm text-fg">
			{change.state === 'rejected' ? 'Rejected' : 'Approved'} by {change.decidedBy}{change.decisionComment
				? `: ${change.decisionComment}`
				: ''}
		</p>
	{/if}

	{#if editable}
		<form
			method="POST"
			action="?/saveChange"
			class="mt-3 flex flex-col gap-3"
			use:enhance={done('Change saved.')}
		>
			<input type="hidden" name="version" value={change.version} />
			<Field label="Type" for="ch-type">
				<Select
					id="ch-type"
					name="changeType"
					value={change.changeType}
					options={[
						{ value: 'standard', label: 'Standard (pre-approved)' },
						{ value: 'normal', label: 'Normal' },
						{ value: 'emergency', label: 'Emergency' }
					]}
				/>
			</Field>
			<fieldset class="flex flex-col gap-2" aria-label="Risk">
				<legend class="text-sm font-medium text-fg">Risk</legend>
				{#each questions as q (q.key)}
					<Field label={q.question} for="risk-{q.key}">
						<Select
							id="risk-{q.key}"
							name="risk.{q.key}"
							value={answers[q.key] ?? ''}
							options={[
								{ value: '', label: 'Choose…' },
								...q.options.map((o) => ({ value: o.key, label: o.label }))
							]}
						/>
					</Field>
				{/each}
			</fieldset>
			{#each [['implementationPlan', 'Implementation plan', change.implementationPlan], ['backoutPlan', 'Backout plan', change.backoutPlan], ['testPlan', 'Test plan', change.testPlan]] as [name, label, value] (name)}
				<Field {label} for="ch-{name}">
					<textarea
						id="ch-{name}"
						{name}
						rows="3"
						class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
						>{value}</textarea
					>
				</Field>
			{/each}
			<div class="grid gap-3 sm:grid-cols-2">
				<Field label="Window starts" for="ch-start">
					<Input
						id="ch-start"
						name="plannedStart"
						type="datetime-local"
						value={local(change.plannedStart)}
					/>
				</Field>
				<Field label="Window ends" for="ch-end">
					<Input
						id="ch-end"
						name="plannedEnd"
						type="datetime-local"
						value={local(change.plannedEnd)}
					/>
				</Field>
			</div>
			{#if assets.length > 0}
				<fieldset class="flex flex-col gap-1">
					<legend class="mb-1 text-sm font-medium text-fg">Affected assets</legend>
					{#each assets as a (a.id)}
						<label class="flex items-center gap-2 text-sm text-fg">
							<input
								type="checkbox"
								name="asset"
								value={a.id}
								checked={change.affectedAssets.includes(a.id)}
							/>
							{a.name}
						</label>
					{/each}
				</fieldset>
			{/if}
			<div class="flex gap-2"><Button type="submit">Save change</Button></div>
		</form>
		<form
			method="POST"
			action="?/submitChange"
			class="mt-2"
			use:enhance={done('Submitted for approval.')}
		>
			<Button type="submit" variant="primary">Submit for approval</Button>
		</form>
	{:else}
		<dl class="mt-3 grid gap-x-4 gap-y-1 text-sm sm:grid-cols-[max-content_1fr]">
			<dt class="text-fg-muted">Window</dt>
			<dd class="text-fg">{when(change.plannedStart)} to {when(change.plannedEnd)}</dd>
			<dt class="text-fg-muted">Implementation</dt>
			<dd class="whitespace-pre-line text-fg">{change.implementationPlan || '–'}</dd>
			<dt class="text-fg-muted">Backout</dt>
			<dd class="whitespace-pre-line text-fg">{change.backoutPlan || '–'}</dd>
		</dl>
	{/if}

	{#if impact && impact.affected.length > 0}
		<div class="mt-3 text-sm" aria-label="Impact" role="region">
			<p class="text-fg">
				<strong>Affects:</strong>
				{impact.affected.map((a) => a.name).join(', ')}
			</p>
			{#if impact.impacted.length > 0}
				<p class="text-fg-muted">
					Also impacted ({impact.impacted.length}): {impact.impacted.map((i) => i.name).join(', ')}
				</p>
			{/if}
		</div>
	{/if}

	{#if canWrite && change.state === 'approved'}
		<form
			method="POST"
			action="?/implementedChange"
			class="mt-3 flex flex-wrap items-end gap-2"
			use:enhance={done('Recorded.')}
		>
			<div class="w-48">
				<Field label="How it went" for="ch-outcome">
					<Select
						id="ch-outcome"
						name="outcome"
						options={[
							{ value: 'successful', label: 'Successful' },
							{ value: 'failed', label: 'Failed' },
							{ value: 'backed_out', label: 'Backed out' }
						]}
					/>
				</Field>
			</div>
			<Button type="submit">Mark implemented</Button>
		</form>
	{/if}
	{#if canWrite && change.state === 'implemented'}
		<form
			method="POST"
			action="?/reviewChange"
			class="mt-3 flex flex-col gap-2"
			use:enhance={done('Review saved.')}
		>
			<Field label="Post-implementation review" for="ch-review">
				<textarea
					id="ch-review"
					name="review"
					rows="3"
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
				></textarea>
			</Field>
			<div><Button type="submit">Save review</Button></div>
		</form>
	{/if}
	{#if change.review}
		<p class="mt-3 text-sm text-fg"><strong>Review:</strong> {change.review}</p>
	{/if}
</Card>
