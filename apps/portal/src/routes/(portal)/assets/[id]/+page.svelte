<script lang="ts">
	import { resolve } from '$app/paths';
	import { Badge, Button, Card } from '@usehuxley/ui';
	import { warrantyText } from '#lib/devices.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let d = $derived(data.device);
	let warranty = $derived(warrantyText(d.warrantyEndsOn));
	let rows = $derived(
		[
			['Type', d.typeName],
			['Serial number', d.serialNumber],
			['Hostname', d.hostname],
			['Make and model', [d.manufacturer, d.model].filter(Boolean).join(' ')],
			['Site', d.siteName],
			['Assigned to', d.contactName]
		].filter((r): r is [string, string] => Boolean(r[1]))
	);
</script>

<svelte:head>
	<title>{d.name} · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<div class="flex flex-wrap items-end justify-between gap-3">
		<div>
			<a href={resolve('/(portal)/assets')} class="text-sm text-accent hover:underline">Devices</a>
			<h1 class="mt-1 text-2xl font-semibold text-fg">{d.name}</h1>
		</div>
		{#if data.me.modules.includes('servicedesk')}
			<Button href="{resolve('/(portal)/tickets/new')}?asset={d.id}" variant="primary">
				Raise a ticket about it
			</Button>
		{/if}
	</div>
	<Card>
		<dl class="grid grid-cols-[auto_1fr] gap-x-6 gap-y-2 text-sm">
			{#each rows as [label, value] (label)}
				<dt class="text-fg-muted">{label}</dt>
				<dd class="text-fg">{value}</dd>
			{/each}
			{#if warranty}
				<dt class="text-fg-muted">Warranty</dt>
				<dd><Badge tone={warranty.tone}>{warranty.text}</Badge></dd>
			{/if}
		</dl>
	</Card>
</div>
