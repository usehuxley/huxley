<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Raise a ticket · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<h1 class="text-2xl font-semibold text-fg">Raise a ticket</h1>
	<Card>
		<form method="POST" class="flex flex-col gap-4" use:enhance>
			{#if data.device}
				<input type="hidden" name="assetId" value={data.device.id} />
				<p class="text-sm text-fg-muted">
					About <span class="font-medium text-fg">{data.device.name}</span>
					{#if data.device.serialNumber}(serial {data.device.serialNumber}){/if}
				</p>
			{/if}
			<Field label="What do you need?" for="title" hint="A short summary">
				<Input id="title" name="title" value={form?.title ?? ''} maxlength={300} required />
			</Field>
			<Field
				label="Details"
				for="description"
				hint="What happened, what you’ve tried, who’s affected"
			>
				<textarea
					id="description"
					name="description"
					rows="6"
					class="w-full rounded-md border border-border bg-surface px-3 py-2 text-sm text-fg"
					>{form?.description ?? ''}</textarea
				>
			</Field>
			{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
			<div><Button type="submit" variant="primary">Raise ticket</Button></div>
		</form>
	</Card>
</div>
