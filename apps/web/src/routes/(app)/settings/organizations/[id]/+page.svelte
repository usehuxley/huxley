<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft, X } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Select, toast } from '@usehuxley/ui';
	import FieldInput from '#lib/fields/FieldInput.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canEdit = $derived(data.user.grantedPermissions.includes('org:update'));
	let fieldErrors = $derived(
		(form && 'fieldErrors' in form ? form.fieldErrors : {}) as Record<string, string>
	);
	let untagged = $derived(data.allTags.filter((t) => !data.tags.some((on) => on.id === t.id)));
	let tagToAdd = $state('');
	type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info';

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update({ reset: false });
			tagToAdd = '';
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
	<title>{data.org.name} · Organizations · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a
		href={resolve('/(app)/settings/organizations')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All organizations
	</a>

	<Card title={data.org.name}>
		{#snippet actions()}
			<Badge tone={data.org.kind === 'provider' ? 'accent' : 'neutral'}>{data.org.kind}</Badge>
		{/snippet}
		<div class="flex flex-wrap items-center gap-2">
			{#each data.tags as tag (tag.id)}
				<span class="inline-flex items-center gap-1">
					<Badge tone={tag.color as Tone}>{tag.name}</Badge>
					{#if canEdit}
						<form method="POST" action="?/tag" use:enhance={notify}>
							<input type="hidden" name="tagId" value={tag.id} />
							<input type="hidden" name="remove" value="true" />
							<button
								type="submit"
								class="text-fg-subtle hover:text-fg"
								aria-label="Remove tag {tag.name}"
							>
								<X size={14} />
							</button>
						</form>
					{/if}
				</span>
			{:else}
				<p class="text-sm text-fg-muted">No tags.</p>
			{/each}
		</div>
		{#if canEdit && untagged.length > 0}
			<form method="POST" action="?/tag" class="mt-4 flex items-end gap-2" use:enhance={notify}>
				<Field label="Add tag" for="tagId">
					<Select
						id="tagId"
						name="tagId"
						bind:value={tagToAdd}
						placeholder="Choose…"
						options={untagged.map((t) => ({ value: t.id, label: t.name }))}
					/>
				</Field>
				<Button type="submit" disabled={!tagToAdd}>Add</Button>
			</form>
		{/if}
	</Card>

	<Card title="Custom fields" description="Fields your administrators defined for organizations.">
		{#if data.fields.length === 0}
			<p class="text-sm text-fg-muted">No custom fields apply to this organization.</p>
		{:else}
			<form method="POST" action="?/fields" class="flex flex-col gap-4" use:enhance={notify}>
				<div class="grid gap-4 sm:grid-cols-2">
					{#each data.fields as field (field.id)}
						<FieldInput {field} disabled={!canEdit} error={fieldErrors[field.key]} />
					{/each}
				</div>
				{#if canEdit}<div><Button type="submit" variant="primary">Save fields</Button></div>{/if}
			</form>
		{/if}
	</Card>
</div>
