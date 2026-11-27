<script lang="ts">
	import { Field, Input, Select } from '@usehuxley/ui';
	import { FORECASTS } from './labels.ts';

	type SelectOption = { value: string; label: string };

	/** An opportunity's editable fields, for creating and changing one. */
	type Values = {
		name?: string;
		description?: string;
		amount?: string;
		recurring?: string;
		currency?: string;
		probabilityOverride?: number | null;
		closeDate?: string | null;
		forecast?: string;
		ownerId?: string | null;
		contactId?: string | null;
		source?: string | null;
	};
	let {
		values = {},
		people,
		contacts = [],
		errors = {},
		stageProbability
	}: {
		values?: Values;
		people: SelectOption[];
		contacts?: SelectOption[];
		errors?: Record<string, string>;
		/** The stage's chance of winning, shown as the default. */
		stageProbability?: number;
	} = $props();
	const textarea =
		'w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg';
</script>

<Field label="Name" for="name" error={errors.name}>
	<Input id="name" name="name" required maxlength={200} value={values.name ?? ''} />
</Field>
<div class="grid gap-4 sm:grid-cols-3">
	<Field label="One-time value" for="amount" error={errors.amount}>
		<Input
			id="amount"
			name="amount"
			inputmode="decimal"
			value={values.amount ?? ''}
			placeholder="0"
		/>
	</Field>
	<Field label="Monthly recurring" for="recurring" error={errors.recurring}>
		<Input
			id="recurring"
			name="recurring"
			inputmode="decimal"
			value={values.recurring ?? ''}
			placeholder="0"
		/>
	</Field>
	<Field label="Currency" for="currency" error={errors.currency}>
		<Input id="currency" name="currency" maxlength={3} value={values.currency ?? 'USD'} />
	</Field>
</div>
<div class="grid gap-4 sm:grid-cols-3">
	<Field label="Expected close" for="closeDate" error={errors.closeDate}>
		<Input id="closeDate" name="closeDate" type="date" value={values.closeDate ?? ''} />
	</Field>
	<Field
		label="Probability %"
		for="probability"
		error={errors.probability}
		hint={stageProbability != null
			? `Blank uses the stage's ${stageProbability}%`
			: 'Blank uses the stage’s'}
	>
		<Input
			id="probability"
			name="probability"
			type="number"
			min={0}
			max={100}
			value={values.probabilityOverride ?? ''}
		/>
	</Field>
	<Field label="Forecast" for="forecast">
		<Select
			id="forecast"
			name="forecast"
			value={values.forecast ?? 'pipeline'}
			options={FORECASTS.map((f) => ({ value: f.value, label: f.label }))}
		/>
	</Field>
</div>
<div class="grid gap-4 sm:grid-cols-3">
	<Field label="Owner" for="ownerId">
		<Select
			id="ownerId"
			name="ownerId"
			value={values.ownerId ?? ''}
			options={[{ value: '', label: 'Me' }, ...people]}
		/>
	</Field>
	{#if contacts.length > 0}
		<Field label="Contact" for="contactId" error={errors.contactId}>
			<Select
				id="contactId"
				name="contactId"
				value={values.contactId ?? ''}
				options={[{ value: '', label: 'None' }, ...contacts]}
			/>
		</Field>
	{/if}
	<Field label="Source" for="source" error={errors.source}>
		<Input
			id="source"
			name="source"
			maxlength={60}
			value={values.source ?? ''}
			placeholder="Referral, web, event…"
		/>
	</Field>
</div>
<Field label="Description" for="description" error={errors.description}>
	<textarea id="description" name="description" rows="4" maxlength={8000} class={textarea}
		>{values.description ?? ''}</textarea
	>
</Field>
