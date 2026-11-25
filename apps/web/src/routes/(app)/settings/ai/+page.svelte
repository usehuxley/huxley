<script lang="ts">
	import { Badge, Card, Field } from '@usehuxley/ui';
	import ProviderCard from '#lib/ai/ProviderCard.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
	const testFor = (use: 'chat' | 'embeddings') =>
		form && 'test' in form && form.test && form.use === use ? form.test : null;
	const when = (v: string) => new Date(v).toLocaleString();
	const tone = (outcome: string) =>
		outcome === 'ok' ? 'success' : outcome === 'refused' ? 'warning' : 'danger';
</script>

<svelte:head>
	<title>AI · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<header class="flex flex-wrap items-end justify-between gap-4">
		<div>
			<h2 class="text-xl font-semibold text-fg">AI</h2>
			<p class="mt-1 text-sm text-fg-muted">
				Your own AI provider and key, for AI workflow steps, and an embeddings provider for
				searching by meaning. Organizations below use them unless they set their own.
			</p>
		</div>
		{#if data.orgs.length > 1}
			<form method="GET" class="flex items-end gap-2">
				<Field label="Organization" for="ai-org">
					<select
						id="ai-org"
						name="org"
						class="rounded-control border border-border bg-surface px-2 py-1.5 text-sm"
						onchange={(e) => e.currentTarget.form?.requestSubmit()}
					>
						{#each data.orgs as org (org.id)}
							<option value={org.id} selected={org.id === data.orgId}>{org.name}</option>
						{/each}
					</select>
				</Field>
			</form>
		{/if}
	</header>
	{#if form && 'message' in form && form.message}
		<p class="text-sm text-fg" role="status">{form.message}</p>
	{/if}

	<ProviderCard
		use="chat"
		title="AI provider"
		empty="Set one below to use AI steps in workflows."
		provider={data.provider}
		orgId={data.orgId}
		{orgName}
		test={testFor('chat')}
	/>

	<ProviderCard
		use="embeddings"
		title="Embeddings provider"
		empty="Set one below to search tickets by meaning. Anthropic has no embeddings, so this is often another vendor."
		provider={data.embeddings}
		orgId={data.orgId}
		{orgName}
		test={testFor('embeddings')}
	/>

	<Card title="Recent AI calls">
		{#if data.calls.length === 0}
			<p class="text-sm text-fg-muted">None yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border text-sm" aria-label="AI calls">
				{#each data.calls as call (call.id)}
					<li class="flex flex-wrap items-center justify-between gap-2 py-2">
						<span>
							<Badge tone={tone(call.outcome)}>{call.outcome}</Badge>
							<span class="ml-1">{call.feature}</span>
							<span class="font-mono text-xs text-fg-muted">{call.model}</span>
						</span>
						<span class="text-xs text-fg-muted">
							{call.inputTokens + call.outputTokens} tokens · {when(call.createdAt)}
						</span>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>
