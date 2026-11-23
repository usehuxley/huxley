<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Badge, Button, Card, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let fieldType = $state('text');
	let canTags = $derived(data.user.grantedPermissions.includes('tag:manage'));
	let orgName = (id: string | null | undefined) => data.orgs.find((o) => o.id === id)?.name;
	type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info';

	const types = [
		{ value: 'text', label: 'Text' },
		{ value: 'long_text', label: 'Long text' },
		{ value: 'number', label: 'Number' },
		{ value: 'boolean', label: 'Yes / no' },
		{ value: 'date', label: 'Date' },
		{ value: 'select', label: 'Choice' },
		{ value: 'multi_select', label: 'Multiple choice' },
		{ value: 'url', label: 'Web address' },
		{ value: 'email', label: 'Email address' }
	];
	const typeLabel = (t: string) => types.find((x) => x.value === t)?.label ?? t;
	const colors = ['neutral', 'accent', 'success', 'warning', 'danger', 'info'].map((c) => ({
		value: c,
		label: c[0].toUpperCase() + c.slice(1)
	}));

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update();
			if (result.type === 'success' || result.type === 'failure') {
				toast.push({
					title: String(result.data?.message ?? 'Done'),
					tone: result.type === 'success' ? 'success' : 'danger'
				});
			}
		};
	};
</script>

<svelte:head>
	<title>Fields & tags · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card title="Organization fields" description="Extra details you track for each organization.">
		{#if data.fields.length === 0}
			<p class="text-sm text-fg-muted">No custom fields yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.fields as field (field.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-medium text-fg">
								<span class={field.archivedAt ? 'line-through opacity-60' : ''}>{field.label}</span>
								<code class="font-mono text-xs text-fg-muted">{field.key}</code>
								{#if field.required}<Badge tone="warning">Required</Badge>{/if}
								{#if field.archivedAt}<Badge>Archived</Badge>{/if}
							</p>
							<p class="mt-0.5 text-sm text-fg-muted">
								{typeLabel(field.fieldType)}{field.scopeOrgId
									? ` · Only ${orgName(field.scopeOrgId) ?? 'one organization'} and below`
									: ''}
							</p>
						</div>
						<form method="POST" action="?/archiveField" use:enhance={notify}>
							<input type="hidden" name="id" value={field.id} />
							<input type="hidden" name="archived" value={String(!field.archivedAt)} />
							<Button
								type="submit"
								size="sm"
								aria-label="{field.archivedAt ? 'Restore' : 'Archive'} {field.label}"
							>
								{field.archivedAt ? 'Restore' : 'Archive'}
							</Button>
						</form>
					</li>
				{/each}
			</ul>
		{/if}
		<form
			method="POST"
			action="?/createField"
			class="mt-6 flex flex-col gap-4 border-t border-border pt-4"
			use:enhance={notify}
		>
			<div class="grid gap-4 sm:grid-cols-2">
				<Field label="Field label" for="fieldLabel">
					<Input
						id="fieldLabel"
						name="label"
						required
						maxlength={100}
						placeholder="e.g. Contract renewal"
					/>
				</Field>
				<Field label="Type" for="fieldType">
					<Select id="fieldType" name="fieldType" bind:value={fieldType} options={types} />
				</Field>
				<Field label="Help text" for="fieldHelp">
					<Input id="fieldHelp" name="help" maxlength={500} />
				</Field>
				<Field label="Only for" for="scopeOrgId" hint="Leave empty to apply to every organization.">
					<Select
						id="scopeOrgId"
						name="scopeOrgId"
						options={[
							{ value: '', label: 'Every organization' },
							...data.orgs.map((o) => ({ value: o.id, label: o.name }))
						]}
					/>
				</Field>
				{#if fieldType === 'select' || fieldType === 'multi_select'}
					<Field label="Options" for="fieldOptions" hint="One per line.">
						<textarea
							id="fieldOptions"
							name="options"
							rows="3"
							required
							class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
						></textarea>
					</Field>
				{/if}
			</div>
			<label class="flex items-center gap-2 text-sm text-fg">
				<input type="checkbox" name="required" /> Required
			</label>
			<div><Button type="submit" variant="primary">Add field</Button></div>
		</form>
	</Card>

	<Card title="Tags" description="Labels you can put on organizations.">
		{#if data.tags.length === 0}
			<p class="text-sm text-fg-muted">No tags yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.tags as tag (tag.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<p class="flex items-center gap-2 text-sm text-fg">
							<Badge tone={tag.color as Tone}>{tag.name}</Badge>
							<span class="text-fg-muted">{tag.usage} record{tag.usage === 1 ? '' : 's'}</span>
						</p>
						{#if canTags}
							<form method="POST" action="?/deleteTag" use:enhance={notify}>
								<input type="hidden" name="id" value={tag.id} />
								<Button type="submit" size="sm" aria-label="Delete tag {tag.name}">Delete</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if canTags}
			<form
				method="POST"
				action="?/createTag"
				class="mt-6 flex flex-wrap items-end gap-2 border-t border-border pt-4"
				use:enhance={notify}
			>
				<Field label="Tag name" for="tagName">
					<Input id="tagName" name="name" required maxlength={50} />
				</Field>
				<Field label="Color" for="tagColor">
					<Select id="tagColor" name="color" value="neutral" options={colors} />
				</Field>
				<Button type="submit" variant="primary">Add tag</Button>
			</form>
		{/if}
	</Card>
</div>
