<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>New quote · Sales · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/sales/quotes')} class="text-sm text-fg-muted hover:text-fg">
			← Quotes
		</a>
		<h1 class="mt-2 text-[26px] font-extrabold tracking-tight text-fg">New quote</h1>
		{#if data.opportunity}
			<p class="mt-1 text-sm text-fg-subtle">
				For {data.opportunity.name} · {data.opportunity.orgName}
			</p>
		{/if}
	</div>
	<Card>
		<form method="POST" class="flex flex-col gap-4" use:enhance>
			{#if data.opportunity}
				<input type="hidden" name="orgId" value={data.opportunity.orgId} />
				<input type="hidden" name="opportunityId" value={data.opportunity.id} />
				{#if data.opportunity.contactId}<input
						type="hidden"
						name="contactId"
						value={data.opportunity.contactId}
					/>{/if}
				<input type="hidden" name="currency" value={data.opportunity.currency} />
			{:else}
				<Field label="Client" for="orgId">
					<Select
						id="orgId"
						name="orgId"
						required
						value={data.orgId}
						placeholder="Choose a client"
						options={data.clients}
					/>
				</Field>
			{/if}
			<Field label="Title" for="title">
				<Input
					id="title"
					name="title"
					required
					maxlength={200}
					value={data.opportunity?.name ?? ''}
				/>
			</Field>
			{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
			<div><Button type="submit" variant="primary">Start the quote</Button></div>
		</form>
	</Card>
</div>
