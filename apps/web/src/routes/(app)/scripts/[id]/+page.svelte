<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import FormRenderer from '#lib/form-renderer/FormRenderer.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let script = $derived(data.script);
	let canManage = $derived(data.user.grantedPermissions.includes('script:manage'));
	let source = $state('');
	let parametersText = $state('');
	$effect(() => {
		source = script.source;
		parametersText = JSON.stringify(script.parameters, null, 2);
	});
	const tryParse = (text: string): unknown => {
		try {
			return JSON.parse(text);
		} catch {
			return undefined;
		}
	};
	let preview = $state<Record<string, unknown>>({});
	let previewSchema = $derived(tryParse(parametersText));
	const when = (v: string) => new Date(v).toLocaleString();
</script>

<svelte:head>
	<title>{script.name} · Scripts · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<header>
		<a
			href={resolve('/(app)/scripts')}
			class="inline-flex items-center gap-1 text-sm text-fg-muted hover:text-fg"
		>
			<ArrowLeft size={14} /> Scripts
		</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">{script.name}</h1>
		<p class="mt-1 text-sm text-fg-muted">
			<span class="font-mono">{script.key}</span> · {script.language === 'pwsh'
				? 'PowerShell 7'
				: script.language === 'javascript'
					? 'JavaScript (Deno)'
					: 'Python 3'}
			· version {script.currentVersion}
		</p>
	</header>
	{#if form?.message}<p class="text-sm text-fg" role="status">{form.message}</p>{/if}

	{#if data.old}
		<Card title="Version {data.old.version} (read-only)">
			<pre
				class="max-h-96 overflow-auto rounded-control bg-surface-sunken p-3 font-mono text-xs text-fg"
				aria-label="Version {data.old.version} source">{data.old.source}</pre>
			<a
				href={resolve('/(app)/scripts/[id]', { id: script.id })}
				class="mt-2 inline-block text-sm text-accent hover:underline"
			>
				Back to the current version
			</a>
		</Card>
	{/if}

	<div class="grid gap-6 lg:grid-cols-3">
		<div class="lg:col-span-2">
			<Card title="Source">
				<form method="POST" action="?/save" use:enhance class="flex flex-col gap-3">
					<input type="hidden" name="baseVersion" value={script.currentVersion} />
					<textarea
						name="source"
						aria-label="Script source"
						rows="22"
						spellcheck="false"
						readonly={!canManage}
						class="rounded-control border border-border bg-surface p-3 font-mono text-xs text-fg"
						bind:value={source}></textarea>
					<label for="parameters" class="text-sm font-medium text-fg">
						Parameters (JSON Schema)
					</label>
					<textarea
						id="parameters"
						name="parameters"
						rows="8"
						spellcheck="false"
						readonly={!canManage}
						class="rounded-control border border-border bg-surface p-3 font-mono text-xs text-fg"
						bind:value={parametersText}></textarea>
					{#if canManage}
						<Field label="What changed" for="notes">
							<Input id="notes" name="notes" maxlength={2000} />
						</Field>
						<div><Button type="submit">Save as version {script.currentVersion + 1}</Button></div>
					{/if}
				</form>
				{#if canManage}
					<form
						method="POST"
						action={script.archived ? '?/restore' : '?/archive'}
						use:enhance
						class="mt-3"
					>
						<Button type="submit" variant="ghost">{script.archived ? 'Restore' : 'Archive'}</Button>
					</form>
				{/if}
			</Card>
		</div>
		<div class="flex flex-col gap-6">
			<Card title="Parameters" description="What a step running it asks for.">
				{#if previewSchema === undefined}
					<p class="text-sm text-fg-muted">Not valid JSON yet.</p>
				{:else}
					<FormRenderer schema={previewSchema} bind:values={preview} idPrefix="param" />
				{/if}
			</Card>
			<Card title="Versions">
				<ul class="flex flex-col gap-2 text-sm" aria-label="Versions">
					{#each data.versions as v (v.version)}
						<li>
							<a
								href="{resolve('/(app)/scripts/[id]', { id: script.id })}?version={v.version}"
								class="font-medium text-accent hover:underline"
							>
								Version {v.version}
							</a>
							<span class="text-fg-muted">· {when(v.createdAt)}</span>
							{#if v.notes}<p class="text-fg-muted">{v.notes}</p>{/if}
						</li>
					{/each}
				</ul>
			</Card>
		</div>
	</div>
</div>
