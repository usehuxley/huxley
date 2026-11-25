<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Link2, X } from '@lucide/svelte';
	import { Button, Card, Input, toast } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { assets }: { assets: components['schemas']['TicketAssets'] } = $props();

	type Found = { id: string; name: string; typeName: string };
	let q = $state('');
	let found: Found[] = $state([]);
	let timer: ReturnType<typeof setTimeout> | undefined;

	async function find(query: string) {
		const res = await fetch(`/assets/find?q=${encodeURIComponent(query)}`);
		if (res.ok && q.trim() === query) {
			const linked = new Set(assets.linked.map((a) => a.id));
			found = ((await res.json()) as Found[]).filter((f) => !linked.has(f.id));
		}
	}
	function search() {
		clearTimeout(timer);
		const query = q.trim();
		if (query.length < 2) {
			found = [];
			return;
		}
		timer = setTimeout(() => void find(query), 250);
	}

	const done: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const message =
				(result.type === 'success' || result.type === 'failure'
					? (result.data as { assets?: { message?: string } } | undefined)?.assets?.message
					: undefined) ?? 'Done.';
			toast.push({ title: message, tone: result.type === 'success' ? 'success' : 'danger' });
			if (result.type === 'success') {
				q = '';
				found = [];
			}
		};
</script>

<Card title="Assets">
	<div class="flex flex-col gap-4 text-sm">
		{#if assets.linked.length > 0}
			<ul class="flex flex-col gap-2" aria-label="Ticket assets">
				{#each assets.linked as a (a.id)}
					<li class="flex items-start justify-between gap-2">
						<div class="min-w-0">
							<a class="text-fg hover:underline" href={resolve('/(app)/assets/[id]', { id: a.id })}>
								{a.name}
							</a>
							<p class="text-xs text-fg-subtle">
								{a.typeName}{a.serialNumber ? ` · ${a.serialNumber}` : ''}
							</p>
						</div>
						{#if assets.canLink}
							<form method="POST" action="?/unlinkAsset" use:enhance={done}>
								<input type="hidden" name="assetId" value={a.id} />
								<button
									type="submit"
									class="text-fg-subtle hover:text-fg"
									aria-label="Remove {a.name}"
								>
									<X size={14} />
								</button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{:else}
			<p class="text-fg-muted">No assets linked.</p>
		{/if}

		{#if assets.canLink && assets.requesterAssets.length > 0}
			<div>
				<p class="mb-1 text-xs font-medium tracking-wide text-fg-subtle uppercase">
					The requester's
				</p>
				<ul class="flex flex-col gap-2" aria-label="Requester assets">
					{#each assets.requesterAssets as a (a.id)}
						<li class="flex items-center justify-between gap-2">
							<span class="min-w-0 truncate text-fg">
								{a.name} <span class="text-xs text-fg-subtle">{a.typeName}</span>
							</span>
							<form method="POST" action="?/linkAsset" use:enhance={done}>
								<input type="hidden" name="assetId" value={a.id} />
								<Button type="submit" size="sm" aria-label="Link {a.name}">
									<Link2 size={14} /> Link
								</Button>
							</form>
						</li>
					{/each}
				</ul>
			</div>
		{/if}

		{#if assets.canLink}
			<div class="flex flex-col gap-2">
				<label for="asset-find" class="sr-only">Find an asset to link</label>
				<Input
					id="asset-find"
					bind:value={q}
					oninput={search}
					placeholder="Find an asset to link"
					autocomplete="off"
				/>
				{#if found.length > 0}
					<ul class="flex flex-col gap-1" aria-label="Assets found">
						{#each found as a (a.id)}
							<li class="flex items-center justify-between gap-2">
								<span class="min-w-0 truncate text-fg">
									{a.name} <span class="text-xs text-fg-subtle">{a.typeName}</span>
								</span>
								<form method="POST" action="?/linkAsset" use:enhance={done}>
									<input type="hidden" name="assetId" value={a.id} />
									<Button type="submit" size="sm" aria-label="Link {a.name}">
										<Link2 size={14} /> Link
									</Button>
								</form>
							</li>
						{/each}
					</ul>
				{/if}
			</div>
		{/if}
	</div>
</Card>
