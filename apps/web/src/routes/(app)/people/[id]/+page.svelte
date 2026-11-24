<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { BadgeCheck, ExternalLink, Plus } from '@lucide/svelte';
	import { Badge, Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import { LEVELS, certState, certTone } from '#lib/workforce/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let person = $derived(data.person);
	let self = $derived(data.user.id === person.userId);
	let manager = $derived(data.user.grantedPermissions.includes('workforce:manage'));
	let canEdit = $derived(self || manager);
	let canVerify = $derived(manager && !self);
	let errors: Record<string, string> = $derived(form?.errors ?? {});

	let adding = $state('');
	let addLevel = $state('2');
	let unrated = $derived(data.skills.filter((s) => !person.skills.some((m) => m.skillId === s.id)));

	type Held = (typeof person.certifications)[number];
	let certOpen = $state(false);
	let editing: Held | null = $state(null);
	function showCert(held: Held | null) {
		editing = held;
		certOpen = true;
	}

	const levelOptions = LEVELS.map((l) => ({
		value: String(l.value),
		label: `${l.value} · ${l.label}`
	}));
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
	/** Submits the enclosing form when a select changes. */
	const submitOnChange = (event: Event) =>
		(event.currentTarget as HTMLSelectElement).form?.requestSubmit();
</script>

<svelte:head>
	<title>{person.name} · People · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/people')} class="text-sm text-fg-muted hover:text-fg">← People</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">{person.name}</h1>
		<p class="text-fg-muted">{person.email}</p>
	</div>

	<Card
		title="Skills"
		description={self
			? 'Rate yourself; a lead can verify your levels.'
			: 'Levels are self-assessed unless verified.'}
	>
		{#if person.skills.length === 0}
			<p class="text-sm text-fg-muted">No skills yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each person.skills as skill (skill.skillId)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-2.5 first:pt-0">
						<div class="min-w-0">
							<p class="text-sm font-medium text-fg">{skill.name}</p>
							<p class="text-xs text-fg-muted">
								{skill.category || 'Other'} ·
								{#if skill.verifiedBy}
									<span class="inline-flex items-center gap-1 text-success">
										<BadgeCheck size={12} /> Verified by {skill.verifiedBy} on {skill.verifiedOn}
									</span>
								{:else}
									Self-assessed
								{/if}
							</p>
						</div>
						{#if canEdit}
							<div class="flex items-center gap-2">
								<form method="POST" action="?/skill" use:enhance={done('Skill saved.')}>
									<input type="hidden" name="skillId" value={skill.skillId} />
									<label class="sr-only" for="level-{skill.skillId}">{skill.name} level</label>
									<Select
										id="level-{skill.skillId}"
										name="level"
										class="w-40"
										value={String(skill.level)}
										options={levelOptions}
										onchange={submitOnChange}
									/>
								</form>
								{#if canVerify && !skill.verifiedBy}
									<form method="POST" action="?/skill" use:enhance={done('Level verified.')}>
										<input type="hidden" name="skillId" value={skill.skillId} />
										<input type="hidden" name="level" value={skill.level} />
										<input type="hidden" name="verified" value="true" />
										<Button size="sm" type="submit">Verify</Button>
									</form>
								{/if}
								<form method="POST" action="?/skill" use:enhance={done('Skill removed.')}>
									<input type="hidden" name="skillId" value={skill.skillId} />
									<Button size="sm" type="submit" variant="ghost" aria-label="Remove {skill.name}">
										Remove
									</Button>
								</form>
							</div>
						{:else}
							<Badge tone="accent">{skill.level} · {LEVELS[skill.level - 1]?.label}</Badge>
						{/if}
					</li>
				{/each}
			</ul>
		{/if}
		{#if canEdit && unrated.length > 0}
			<form
				method="POST"
				action="?/skill"
				class="mt-4 flex flex-wrap items-end gap-2 border-t border-border pt-4"
				use:enhance={done('Skill added.', () => (adding = ''))}
			>
				<Field label="Add a skill" for="add-skill">
					<Select
						id="add-skill"
						name="skillId"
						class="w-64"
						bind:value={adding}
						placeholder="Choose a skill"
						options={unrated.map((s) => ({
							value: s.id,
							label: s.category ? `${s.name} (${s.category})` : s.name
						}))}
					/>
				</Field>
				<Field label="Level" for="add-level">
					<Select
						id="add-level"
						name="level"
						class="w-40"
						bind:value={addLevel}
						options={levelOptions}
					/>
				</Field>
				<Button type="submit" disabled={!adding}><Plus size={16} /> Add</Button>
			</form>
		{/if}
	</Card>

	<Card title="Certifications">
		{#snippet actions()}
			{#if canEdit && data.certifications.length > 0}
				<Button size="sm" onclick={() => showCert(null)}><Plus size={16} /> Add</Button>
			{/if}
		{/snippet}
		{#if person.certifications.length === 0}
			<p class="text-sm text-fg-muted">No certifications yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each person.certifications as held (held.id)}
					<li class="flex flex-wrap items-center justify-between gap-3 py-2.5 first:pt-0">
						<div class="min-w-0">
							<p class="text-sm font-medium text-fg">
								{held.vendor}
								{held.name}
								{#if held.code}<span class="text-fg-muted">({held.code})</span>{/if}
							</p>
							<p class="text-xs text-fg-muted">
								Earned {held.earnedOn}{held.expiresOn
									? ` · expires ${held.expiresOn}`
									: ' · doesn’t expire'}{held.credentialId ? ` · ${held.credentialId}` : ''}
								<!-- eslint-disable svelte/no-navigation-without-resolve -- an external link -->
								{#if held.verifyUrl}
									· <a
										href={held.verifyUrl}
										target="_blank"
										rel="noopener noreferrer"
										class="inline-flex items-center gap-0.5 text-accent hover:underline"
									>
										Check it <ExternalLink size={11} />
									</a>
								{/if}
								<!-- eslint-enable svelte/no-navigation-without-resolve -->
							</p>
						</div>
						<div class="flex items-center gap-2">
							<Badge tone={certTone(held.state)}>{certState(held.state)}</Badge>
							{#if canEdit}
								<Button size="sm" onclick={() => showCert(held)}>Edit</Button>
								<form
									method="POST"
									action="?/removeCertification"
									use:enhance={done('Certification removed.')}
								>
									<input type="hidden" name="heldId" value={held.id} />
									<Button size="sm" type="submit" variant="ghost">Remove</Button>
								</form>
							{/if}
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>
</div>

<Dialog
	bind:open={certOpen}
	title={editing ? `Renew or correct ${editing.name}` : 'Add a certification'}
>
	<form
		method="POST"
		action={editing ? '?/updateCertification' : '?/addCertification'}
		class="flex flex-col gap-4"
		use:enhance={done('Certification saved.', () => (certOpen = false))}
	>
		{#if editing}
			<input type="hidden" name="heldId" value={editing.id} />
		{:else}
			<Field label="Certification" for="cert" error={errors.certificationId}>
				<Select
					id="cert"
					name="certificationId"
					value=""
					placeholder="Choose one"
					required
					options={data.certifications.map((c) => ({
						value: c.id,
						label: `${c.vendor} ${c.name}${c.code ? ` (${c.code})` : ''}`
					}))}
				/>
			</Field>
		{/if}
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Earned" for="earned" error={errors.earnedOn}>
				<Input id="earned" name="earnedOn" type="date" required value={editing?.earnedOn ?? ''} />
			</Field>
			<Field
				label="Expires"
				for="expires"
				error={errors.expiresOn}
				hint="Empty: from the certification’s validity."
			>
				<Input id="expires" name="expiresOn" type="date" value={editing?.expiresOn ?? ''} />
			</Field>
			<Field label="Credential ID" for="credential" error={errors.credentialId}>
				<Input id="credential" name="credentialId" value={editing?.credentialId ?? ''} />
			</Field>
			<Field label="Verification link" for="verify" error={errors.verifyUrl}>
				<Input
					id="verify"
					name="verifyUrl"
					type="url"
					placeholder="https://"
					value={editing?.verifyUrl ?? ''}
				/>
			</Field>
		</div>
		<div class="flex justify-end gap-2">
			<Button onclick={() => (certOpen = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Save</Button>
		</div>
	</form>
</Dialog>
