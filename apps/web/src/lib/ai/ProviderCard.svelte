<script lang="ts">
	import { enhance } from '$app/forms';
	import { Badge, Button, Card, EmptyState, Field, Input, Select } from '@usehuxley/ui';
	import { Sparkles } from '@lucide/svelte';
	import type { components } from '@usehuxley/api-client';

	type Use = 'chat' | 'embeddings';
	type Test = { ok: boolean; message: string; model?: string | null };

	let {
		use,
		title,
		empty,
		provider,
		orgId,
		orgName,
		test
	}: {
		use: Use;
		title: string;
		/** What the empty state says. */
		empty: string;
		provider: components['schemas']['AiProviderState'];
		orgId: string;
		orgName: Map<string, string>;
		/** This card's last test result. */
		test?: Test | null;
	} = $props();

	const kinds = $derived(
		use === 'chat'
			? [
					{ value: 'anthropic', label: 'Anthropic' },
					{ value: 'openai', label: 'OpenAI' },
					{ value: 'azure_openai', label: 'Azure OpenAI' },
					{ value: 'openai_compatible', label: 'OpenAI-compatible' }
				]
			: [
					{ value: 'openai', label: 'OpenAI' },
					{ value: 'voyage', label: 'Voyage AI' },
					{ value: 'azure_openai', label: 'Azure OpenAI' },
					{ value: 'openai_compatible', label: 'OpenAI-compatible' }
				]
	);
	const CHAT_DEFAULTS: Record<string, string> = { anthropic: 'claude-opus-5-5' };
	const EMBEDDING_DEFAULTS: Record<string, string> = {
		openai: 'text-embedding-3-small',
		voyage: 'voyage-3.5'
	};
	let defaults = $derived(use === 'chat' ? CHAT_DEFAULTS : EMBEDDING_DEFAULTS);
	const id = (name: string) => `${use}-${name}`;
	const kindLabel = (k: string) => kinds.find((x) => x.value === k)?.label ?? k;

	let noun = $derived(use === 'chat' ? 'AI provider' : 'embeddings provider');
	let own = $derived(provider.own);
	let effective = $derived(provider.effective);
	let inherited = $derived(effective && effective.orgId !== orgId);
	let kind = $derived(own?.kind ?? kinds[0].value);
	let needsUrl = $derived(kind === 'azure_openai' || kind === 'openai_compatible');
	let budget = $derived(effective?.monthlyTokenBudget);
</script>

<div role="region" aria-label={title}>
	<Card {title}>
		{#if effective}
			<div class="mb-4 flex flex-wrap items-center gap-2 text-sm" data-testid="{use}-effective">
				<Badge tone="info">{kindLabel(effective.kind)}</Badge>
				<span class="font-mono text-xs">{effective.model}</span>
				{#if inherited}
					<span class="text-fg-muted">
						from {orgName.get(effective.orgId) ?? 'an organization above'}
					</span>
				{/if}
				<span class="text-fg-muted">
					· {provider.usedThisMonth.toLocaleString()} tokens this month{budget
						? ` of ${budget.toLocaleString()}`
						: ''}
				</span>
			</div>
		{:else}
			<EmptyState title="No {noun}" description={empty}>
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
			<input type="hidden" name="orgId" value={orgId} />
			<input type="hidden" name="use" value={use} />
			<p class="text-sm font-medium text-fg">
				{own ? `This organization's ${noun}` : `Set an ${noun} for this organization`}
			</p>
			<div class="grid gap-3 sm:grid-cols-2">
				<Field label="Provider" for={id('kind')}>
					<Select id={id('kind')} name="kind" options={kinds} bind:value={kind} />
				</Field>
				<Field
					label="Model"
					for={id('model')}
					hint={defaults[kind] ? `Default: ${defaults[kind]}` : 'Required'}
				>
					<Input id={id('model')} name="model" value={own?.model ?? ''} maxlength={200} />
				</Field>
				{#if needsUrl}
					<Field
						label="Base URL"
						for={id('url')}
						hint={kind === 'azure_openai' ? 'https://<resource>.openai.azure.com' : ''}
					>
						<Input id={id('url')} name="baseUrl" value={own?.baseUrl ?? ''} required />
					</Field>
				{/if}
				{#if kind === 'azure_openai'}
					<Field label="Deployment" for={id('deployment')} hint="Default: the model">
						<Input id={id('deployment')} name="deployment" value={own?.deployment ?? ''} />
					</Field>
				{/if}
				<Field
					label="API key"
					for={id('key')}
					hint={own ? 'Leave empty to keep the current key' : 'Stored encrypted; never shown again'}
				>
					<Input id={id('key')} name="apiKey" type="password" autocomplete="off" required={!own} />
				</Field>
				<Field label="Monthly token budget" for={id('budget')} hint="Optional: tokens used">
					<Input
						id={id('budget')}
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
					<input type="hidden" name="orgId" value={orgId} />
					<input type="hidden" name="use" value={use} />
					<Button type="submit" variant="secondary">Test</Button>
				</form>
			{/if}
			{#if own}
				<form method="POST" action="?/remove" use:enhance>
					<input type="hidden" name="orgId" value={orgId} />
					<input type="hidden" name="use" value={use} />
					<Button type="submit" variant="danger">Remove</Button>
				</form>
			{/if}
		</div>
		{#if test}
			<p
				class="mt-3 text-sm {test.ok ? 'text-success' : 'text-danger'}"
				role="status"
				data-testid="{use}-test"
			>
				{test.ok
					? use === 'chat'
						? `Working: ${test.model} said “${test.message}”`
						: `Working: ${test.model} gave ${test.message}`
					: test.message}
			</p>
		{/if}
	</Card>
</div>
