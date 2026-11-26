<script lang="ts">
	import { resolve } from '$app/paths';
	import { Boxes } from '@lucide/svelte';
	import { Badge, Button, Card, EmptyState, Input } from '@usehuxley/ui';
	import { warrantyText } from '#lib/devices.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let company = $derived(data.me.permissions.includes('portal.asset:read'));
</script>

<svelte:head>
	<title>Devices · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Devices</h1>
		<p class="mt-1 text-fg-muted">
			{company ? `${data.me.orgName}’s devices.` : 'The devices assigned to you.'}
		</p>
	</div>
	<form method="GET" class="flex gap-2" role="search">
		<Input name="q" value={data.q} placeholder="Name, serial number or model" aria-label="Search" />
		<Button type="submit">Search</Button>
	</form>
	<Card>
		{#if data.devices.length === 0}
			<EmptyState
				title={data.q ? 'Nothing matches' : 'No devices'}
				description={data.q ? 'Try other words.' : 'None are listed for you yet.'}
			>
				{#snippet icon()}<Boxes size={24} />{/snippet}
			</EmptyState>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Devices">
				{#each data.devices as d (d.id)}
					{@const warranty = warrantyText(d.warrantyEndsOn)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0">
						<div class="min-w-0">
							<a
								href={resolve('/(portal)/assets/[id]', { id: d.id })}
								class="font-medium text-fg hover:underline"
							>
								{d.name}
							</a>
							<p class="text-xs text-fg-muted">
								{[d.typeName, [d.manufacturer, d.model].filter(Boolean).join(' '), d.contactName]
									.filter(Boolean)
									.join(' · ')}
							</p>
						</div>
						{#if warranty}<Badge tone={warranty.tone}>{warranty.text}</Badge>{/if}
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
