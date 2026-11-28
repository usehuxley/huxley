<script lang="ts">
	import ScanBadge from '#lib/files/ScanBadge.svelte';
	import HealthModelCard from '#lib/orgs/HealthModelCard.svelte';
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft, X } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Select, toast } from '@usehuxley/ui';
	import FieldInput from '#lib/fields/FieldInput.svelte';
	import OrgSettings from '#lib/orgs/OrgSettings.svelte';
	import OrgVariables from '#lib/orgs/OrgVariables.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let canEdit = $derived(data.user.grantedPermissions.includes('org:update'));
	let canConfigureDesk = $derived(data.user.grantedPermissions.includes('servicedesk:configure'));
	let canManageModules = $derived(data.user.grantedPermissions.includes('platform:modules:manage'));
	let fieldErrors = $derived(
		(form && 'fieldErrors' in form ? form.fieldErrors : {}) as Record<string, string>
	);
	let untagged = $derived(data.allTags.filter((t) => !data.tags.some((on) => on.id === t.id)));
	let tagToAdd = $state('');
	const dateFormat = new Intl.DateTimeFormat(undefined, { dateStyle: 'medium' });
	const formatSize = (bytes: number) =>
		bytes < 1024
			? `${bytes} B`
			: bytes < 1024 * 1024
				? `${(bytes / 1024).toFixed(1)} KB`
				: `${(bytes / 1024 / 1024).toFixed(1)} MB`;
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

	<Card
		title="Attachments"
		description="Contracts, diagrams, and other files for this organization."
	>
		<div class="flex flex-col gap-4">
			{#if data.attachments.length === 0}
				<p class="text-sm text-fg-muted">No files yet.</p>
			{:else}
				<ul class="flex flex-col divide-y divide-border">
					{#each data.attachments as file (file.id)}
						<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
							<div class="min-w-0">
								<div class="flex flex-wrap items-center gap-2">
									{#if file.scanStatus === 'infected'}
										<span class="text-sm font-medium text-fg-muted">{file.filename}</span>
									{:else}
										<a
											href={resolve('/(app)/attachments/[id]', { id: file.id })}
											data-sveltekit-reload
											class="text-sm font-medium text-fg hover:underline"
										>
											{file.filename}
										</a>
									{/if}
									<ScanBadge status={file.scanStatus} />
								</div>
								<p class="mt-0.5 text-sm text-fg-muted">
									{formatSize(file.sizeBytes)} · {file.createdByName ?? 'Someone'} · {dateFormat.format(
										new Date(file.createdAt)
									)}
								</p>
							</div>
							{#if canEdit}
								<form method="POST" action="?/deleteAttachment" use:enhance={notify}>
									<input type="hidden" name="id" value={file.id} />
									<Button type="submit" size="sm" aria-label="Delete {file.filename}">
										Delete
									</Button>
								</form>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
			{#if canEdit}
				<form
					method="POST"
					action="?/upload"
					enctype="multipart/form-data"
					class="flex flex-wrap items-end gap-2"
					use:enhance={notify}
				>
					<Field label="File" for="file">
						<input id="file" name="file" type="file" required class="text-sm text-fg" />
					</Field>
					<Button type="submit">Upload</Button>
				</form>
			{/if}
		</div>
	</Card>

	<Card
		title="Organization settings"
		description="Values apply here and to every sub-organization unless they set their own. Lock a value to stop them."
	>
		<OrgSettings
			settings={data.settings}
			orgId={data.org.id}
			canManage={data.user.grantedPermissions.includes('org:settings:manage')}
			{notify}
		/>
		{#if data.healthModel}
			<HealthModelCard
				weights={data.healthModel.weights}
				message={form && 'health' in form ? (form.health as string) : null}
			/>
		{/if}
	</Card>

	{#if data.variables}
		<Card
			title="Workflow variables"
			description="Values workflows read here and in every sub-organization, unless one sets its own. Secrets are encrypted and never shown again."
		>
			<OrgVariables
				variables={data.variables}
				orgId={data.org.id}
				canManage={data.user.grantedPermissions.includes('variable:manage')}
				{notify}
			/>
		</Card>
	{/if}

	{#if data.org.kind === 'provider' && data.org.parentId && data.modules.some((m) => m.enabled)}
		<Card
			title="Modules"
			description="Turn off modules this provider and its clients don't use. Modules off for the whole instance aren't listed."
		>
			<ul class="flex flex-col divide-y divide-border">
				{#each data.modules.filter((m) => m.enabled) as m (m.key)}
					{@const own = m.narrowedBy?.orgId === data.org.id}
					<li
						class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0"
						data-testid="module-{m.key}"
					>
						<div class="min-w-0">
							<p class="text-sm font-medium text-fg">{m.name}</p>
							<p class="text-sm text-fg-muted">
								{#if m.enabledHere}On{:else if own}Off here and below{:else}Off: turned off by {m
										.narrowedBy?.orgName}{/if}
							</p>
						</div>
						{#if canManageModules && (m.enabledHere || own)}
							<form method="POST" action="?/module" use:enhance={notify}>
								<input type="hidden" name="key" value={m.key} />
								<input type="hidden" name="enabled" value={m.enabledHere ? 'false' : 'true'} />
								<Button
									type="submit"
									size="sm"
									aria-label="{m.enabledHere ? 'Turn off' : 'Turn on'} {m.name}"
								>
									{m.enabledHere ? 'Turn off here' : 'Turn back on'}
								</Button>
							</form>
						{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if data.sla}
		{@const sla = data.sla}
		<Card
			title="SLA policy"
			description="The service desk targets for tickets here and in the organizations below."
		>
			<form
				method="POST"
				action="?/sla"
				class="flex flex-wrap items-end gap-2"
				use:enhance={notify}
			>
				<Field
					label="Policy"
					for="sla-policy"
					hint={sla.policyId
						? 'Set here.'
						: sla.source === 'above'
							? `Follows ${sla.effectivePolicyName} from an organization above.`
							: `Follows the default, ${sla.effectivePolicyName}.`}
				>
					<Select
						id="sla-policy"
						name="policyId"
						value={sla.policyId ?? ''}
						disabled={!canConfigureDesk}
						options={[
							{
								value: '',
								label:
									sla.source === 'here'
										? 'Follow the one above'
										: `Follow ${sla.effectivePolicyName}`
							},
							...data.slaPolicies.map((p) => ({ value: p.id, label: p.name }))
						]}
					/>
				</Field>
				{#if canConfigureDesk}<Button type="submit">Save</Button>{/if}
			</form>
		</Card>
	{/if}

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
