<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Plus } from '@lucide/svelte';
	import { Button, Card, Dialog, Field, Input, toast } from '@usehuxley/ui';
	import { byCategory } from '#lib/workforce/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	type Skill = (typeof data.skills)[number];
	type Cert = (typeof data.certifications)[number];
	let skillOpen = $state(false);
	let skill: Skill | null = $state(null);
	let certOpen = $state(false);
	let cert: Cert | null = $state(null);
	let errors: Record<string, string> = $derived(form?.errors ?? {});
	let groups = $derived(byCategory(data.skills));
	let categories = $derived([...new Set(data.skills.map((s) => s.category).filter(Boolean))]);

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>Skills & certifications · Settings · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<Card
		title="Skills"
		description="What your team can do. People rate themselves from learning to expert; leads verify."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					skill = null;
					skillOpen = true;
				}}
			>
				<Plus size={16} /> New skill
			</Button>
		{/snippet}
		{#if data.skills.length === 0}
			<p class="text-sm text-fg-muted">No skills yet.</p>
		{:else}
			<div class="flex flex-col gap-4">
				{#each groups as [category, skills] (category)}
					<section>
						<h3 class="text-xs font-medium tracking-wide text-fg-muted uppercase">{category}</h3>
						<ul class="mt-1 flex flex-col divide-y divide-border">
							{#each skills as s (s.id)}
								<li class="flex items-center justify-between gap-3 py-2">
									<div class="min-w-0">
										<p class="text-sm text-fg">{s.name}</p>
										<p class="text-xs text-fg-muted">
											{s.people}
											{s.people === 1 ? 'person' : 'people'}{s.description
												? ` · ${s.description}`
												: ''}
										</p>
									</div>
									<div class="flex gap-2">
										<Button
											size="sm"
											onclick={() => {
												skill = s;
												skillOpen = true;
											}}
										>
											Edit
										</Button>
										<form
											method="POST"
											action="?/archiveSkill"
											use:enhance={done('Skill archived.')}
										>
											<input type="hidden" name="id" value={s.id} />
											<Button size="sm" type="submit" variant="ghost">Archive</Button>
										</form>
									</div>
								</li>
							{/each}
						</ul>
					</section>
				{/each}
			</div>
		{/if}
	</Card>

	<Card
		title="Certifications"
		description="Vendor certifications your people hold. Expiry is announced 30 days ahead and when it lapses."
	>
		{#snippet actions()}
			<Button
				onclick={() => {
					cert = null;
					certOpen = true;
				}}
			>
				<Plus size={16} /> New certification
			</Button>
		{/snippet}
		{#if data.certifications.length === 0}
			<p class="text-sm text-fg-muted">No certifications yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.certifications as c (c.id)}
					<li class="flex items-center justify-between gap-3 py-2 first:pt-0">
						<div class="min-w-0">
							<p class="text-sm text-fg">
								{c.vendor}
								{c.name}
								{#if c.code}<span class="text-fg-muted">({c.code})</span>{/if}
							</p>
							<p class="text-xs text-fg-muted">
								{c.holders} holding it · {c.validityMonths
									? `valid ${c.validityMonths} months`
									: 'doesn’t expire'}
							</p>
						</div>
						<div class="flex gap-2">
							<Button
								size="sm"
								onclick={() => {
									cert = c;
									certOpen = true;
								}}
							>
								Edit
							</Button>
							<form
								method="POST"
								action="?/archiveCertification"
								use:enhance={done('Certification archived.')}
							>
								<input type="hidden" name="id" value={c.id} />
								<Button size="sm" type="submit" variant="ghost">Archive</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog bind:open={skillOpen} title={skill ? `Edit ${skill.name}` : 'New skill'}>
	<form
		method="POST"
		action="?/skill"
		class="flex flex-col gap-4"
		use:enhance={done('Skill saved.', () => (skillOpen = false))}
	>
		{#if skill}
			<input type="hidden" name="id" value={skill.id} />
			<input type="hidden" name="version" value={skill.version} />
		{/if}
		<Field label="Name" for="skill-name" error={errors.name}>
			<Input id="skill-name" name="name" required maxlength={100} value={skill?.name ?? ''} />
		</Field>
		<Field label="Category" for="skill-category" hint="e.g. Microsoft 365, Networking, Security">
			<Input
				id="skill-category"
				name="category"
				maxlength={100}
				list="skill-categories"
				value={skill?.category ?? ''}
			/>
			<datalist id="skill-categories">
				{#each categories as c (c)}<option value={c}></option>{/each}
			</datalist>
		</Field>
		<Field label="Description" for="skill-description">
			<Input id="skill-description" name="description" value={skill?.description ?? ''} />
		</Field>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (skillOpen = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>

<Dialog bind:open={certOpen} title={cert ? `Edit ${cert.name}` : 'New certification'}>
	<form
		method="POST"
		action="?/certification"
		class="flex flex-col gap-4"
		use:enhance={done('Certification saved.', () => (certOpen = false))}
	>
		{#if cert}
			<input type="hidden" name="id" value={cert.id} />
			<input type="hidden" name="version" value={cert.version} />
		{/if}
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Vendor" for="cert-vendor" error={errors.vendor}>
				<Input
					id="cert-vendor"
					name="vendor"
					required
					value={cert?.vendor ?? ''}
					placeholder="Microsoft"
				/>
			</Field>
			<Field label="Code" for="cert-code" error={errors.code}>
				<Input id="cert-code" name="code" value={cert?.code ?? ''} placeholder="AZ-104" />
			</Field>
		</div>
		<Field label="Name" for="cert-name" error={errors.name}>
			<Input
				id="cert-name"
				name="name"
				required
				value={cert?.name ?? ''}
				placeholder="Azure Administrator"
			/>
		</Field>
		<Field
			label="Valid for (months)"
			for="cert-validity"
			error={errors.validityMonths}
			hint="Empty: it doesn’t expire."
		>
			<Input
				id="cert-validity"
				name="validityMonths"
				type="number"
				min={1}
				max={240}
				value={cert?.validityMonths ? String(cert.validityMonths) : ''}
			/>
		</Field>
		<Field label="Description" for="cert-description">
			<Input id="cert-description" name="description" value={cert?.description ?? ''} />
		</Field>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (certOpen = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
