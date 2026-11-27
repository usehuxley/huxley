<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
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
</div>
