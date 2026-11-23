<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import type { components } from '@usehuxley/api-client';
	import { Badge, Button, Field, Input, Select } from '@usehuxley/ui';

	type Variable = components['schemas']['ResolvedVariable'];

	let {
		variables,
		orgId,
		canManage,
		notify
	}: { variables: Variable[]; orgId: string; canManage: boolean; notify: SubmitFunction } =
		$props();

	const types = [
		{ value: 'string', label: 'Text' },
		{ value: 'secret', label: 'Secret' },
		{ value: 'number', label: 'Number' },
		{ value: 'bool', label: 'True/false' },
		{ value: 'json', label: 'JSON' }
	];
	let kind = $state('string');

	function origin(v: Variable): string {
		if (v.lockedBy) return `Locked by ${v.lockedBy.name}`;
		if (v.source.id === orgId) return v.lockedHere ? 'Set here and locked below' : 'Set here';
		return `Inherited from ${v.source.name}`;
	}
	function shown(v: Variable): string {
		if (v.type === 'secret') return '••••••••';
		return typeof v.value === 'string' ? v.value : JSON.stringify(v.value);
	}
	function usage(v: Variable): string {
		return v.type === 'secret' ? `{{ secrets.${v.key} }}` : `{{ vars.${v.key} }}`;
	}
</script>

<div class="flex flex-col gap-4">
	{#if variables.length === 0}
		<p class="text-sm text-fg-muted">No variables yet.</p>
	{:else}
		<ul class="flex flex-col divide-y divide-border" aria-label="Variables">
			{#each variables as v (v.key)}
				<li class="flex flex-wrap items-center justify-between gap-3 py-3 first:pt-0 last:pb-0">
					<div class="min-w-0">
						<p class="text-sm font-medium text-fg">
							{v.key}
							{#if v.type === 'secret'}<Badge tone="warning">Secret</Badge>{/if}
						</p>
						<p class="mt-0.5 font-mono text-xs break-all text-fg-muted">{shown(v)}</p>
						<p class="mt-0.5 text-xs text-fg-muted">{origin(v)} · <code>{usage(v)}</code></p>
					</div>
					{#if canManage && v.setHere && !v.lockedBy}
						<form method="POST" action="?/clearVariable" use:enhance={notify}>
							<input type="hidden" name="key" value={v.key} />
							<Button type="submit" variant="ghost" size="sm">Remove</Button>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
	{/if}

	{#if canManage}
		<form
			method="POST"
			action="?/variable"
			class="flex flex-wrap items-end gap-3 border-t border-border pt-4"
			use:enhance={notify}
		>
			<Field label="Variable" for="variable-key">
				<Input
					id="variable-key"
					name="key"
					required
					pattern="[a-z][a-z0-9_]*"
					maxlength={64}
					placeholder="m365_domain"
				/>
			</Field>
			<Field label="Type" for="variable-type">
				<Select id="variable-type" name="type" options={types} bind:value={kind} />
			</Field>
			<Field label="Value" for="variable-value">
				<Input
					id="variable-value"
					name="value"
					type={kind === 'secret' ? 'password' : 'text'}
					autocomplete="off"
					required
				/>
			</Field>
			<label class="flex items-center gap-2 pb-2 text-sm text-fg">
				<input type="checkbox" name="locked" /> Lock below
			</label>
			<Button type="submit">Set</Button>
		</form>
	{/if}
</div>
