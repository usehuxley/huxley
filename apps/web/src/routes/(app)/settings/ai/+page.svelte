<script lang="ts">
	import { enhance } from '$app/forms';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { Sparkles } from '@lucide/svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let own = $derived(data.provider.own);
	let effective = $derived(data.provider.effective);
	let inherited = $derived(effective && effective.orgId !== data.orgId);
	let orgName = $derived(new Map(data.orgs.map((o) => [o.id, o.name])));
	const kinds = [
		{ value: 'anthropic', label: 'Anthropic' },
		{ value: 'openai', label: 'OpenAI' },
		{ value: 'azure_openai', label: 'Azure OpenAI' },
		{ value: 'openai_compatible', label: 'OpenAI-compatible' }
	];
	const kindLabel = (k: string) => kinds.find((x) => x.value === k)?.label ?? k;
	let kind = $derived(own?.kind ?? 'anthropic');
	let needsUrl = $derived(kind === 'azure_openai' || kind === 'openai_compatible');
	let budget = $derived(effective?.monthlyTokenBudget);
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
				Your own AI provider and key, for AI workflow steps. Organizations below use it unless they
				set their own.
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

	<Card title="Provider">
		{#if effective}
			<div class="mb-4 flex flex-wrap items-center gap-2 text-sm" data-testid="ai-effective">
				<Badge tone="info">{kindLabel(effective.kind)}</Badge>
				<span class="font-mono text-xs">{effective.model}</span>
				{#if inherited}
					<span class="text-fg-muted">
						from {orgName.get(effective.orgId) ?? 'an organization above'}
					</span>
				{/if}
				<span class="text-fg-muted">
					· {data.provider.usedThisMonth.toLocaleString()} tokens this month{budget
						? ` of ${budget.toLocaleString()}`
						: ''}
				</span>
			</div>
		{:else}
			<EmptyState title="No AI provider" description="Set one below to use AI steps in workflows.">
				{#snippet icon()}<Sparkles size={20} />{/snippet}
			</EmptyState>
		{/if}

		<form
			method="POST"
			action="?/save"
			use:enhance={() =>
				async ({ formElement, update }) => {
					await update({ reset: false });
					// The key is never kept on the page once saved.
					const key = formElement.elements.namedItem('apiKey');
					if (key instanceof HTMLInputElement) key.value = '';
				}}
			class="mt-4 flex flex-col gap-3"
		>
			<input type="hidden" name="orgId" value={data.orgId} />
			<p class="text-sm font-medium text-fg">
				{own ? "This organization's provider" : 'Set a provider for this organization'}
			</p>
			<div class="grid gap-3 sm:grid-cols-2">
				<Field label="Provider" for="ai-kind">
					<Select id="ai-kind" name="kind" options={kinds} bind:value={kind} />
				</Field>
				<Field
					label="Model"
					for="ai-model"
					hint={kind === 'anthropic' ? 'Default: claude-opus-5-5' : 'Required'}
				>
					<Input id="ai-model" name="model" value={own?.model ?? ''} maxlength={200} />
				</Field>
				{#if needsUrl}
					<Field
						label="Base URL"
						for="ai-url"
						hint={kind === 'azure_openai' ? 'https://<resource>.openai.azure.com' : ''}
					>
						<Input id="ai-url" name="baseUrl" value={own?.baseUrl ?? ''} required />
					</Field>
				{/if}
				{#if kind === 'azure_openai'}
					<Field label="Deployment" for="ai-deployment" hint="Default: the model">
						<Input id="ai-deployment" name="deployment" value={own?.deployment ?? ''} />
					</Field>
				{/if}
				<Field
					label="API key"
					for="ai-key"
					hint={own ? 'Leave empty to keep the current key' : 'Stored encrypted; never shown again'}
				>
					<Input id="ai-key" name="apiKey" type="password" autocomplete="off" required={!own} />
				</Field>
				<Field label="Monthly token budget" for="ai-budget" hint="Optional: tokens in and out">
					<Input
						id="ai-budget"
						name="budget"
						type="number"
						min="1"
						value={own?.monthlyTokenBudget ?? ''}
					/>
				</Field>
			</div>
			<div><Button type="submit">Save</Button></div>
		</form>

		<div class="mt-4 flex flex-wrap gap-2">
			{#if effective}
				<form method="POST" action="?/test" use:enhance>
					<input type="hidden" name="orgId" value={data.orgId} />
					<Button type="submit" variant="secondary">Test</Button>
				</form>
			{/if}
			{#if own}
				<form method="POST" action="?/remove" use:enhance>
					<input type="hidden" name="orgId" value={data.orgId} />
					<Button type="submit" variant="danger">Remove</Button>
				</form>
			{/if}
		</div>
		{#if form && 'test' in form && form.test}
			<p
				class="mt-3 text-sm {form.test.ok ? 'text-success' : 'text-danger'}"
				role="status"
				data-testid="ai-test"
			>
				{form.test.ok
					? `Working: ${form.test.model} said “${form.test.message}”`
					: form.test.message}
			</p>
		{/if}
	</Card>

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
