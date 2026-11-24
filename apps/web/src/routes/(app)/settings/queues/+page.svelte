<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, Dialog, Field, Input, Select, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	type Queue = (typeof data.queues)[number];
	let editing: Queue | null = $state(null);
	let open = $state(false);
	let owner = $state('');
	$effect(() => {
		if (!owner && data.providers.length > 0) owner = data.providers[0].value;
	});

	const personName = (id: string) => data.people.find((p) => p.value === id)?.label ?? 'Someone';
	const teamName = (id: string) => data.teams.find((t) => t.value === id)?.label ?? 'A team';

	function edit(queue: Queue) {
		editing = queue;
		open = true;
	}

	const done =
		(message: string, after?: () => void): SubmitFunction =>
		({ formElement }) =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				formElement.reset();
				after?.();
			} else if (result.type === 'failure') {
				const failed = result.data as { message?: string } | undefined;
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
</script>

<svelte:head>
	<title>Service queues · Settings · Huxley</title>
</svelte:head>

{#snippet members(queue: Queue | null)}
	<fieldset class="flex flex-col gap-2">
		<legend class="text-sm font-medium text-fg">Members</legend>
		{#if data.people.length === 0 && data.teams.length === 0}
			<p class="text-sm text-fg-muted">No people or teams to add.</p>
		{/if}
		<div class="flex max-h-56 flex-col gap-1.5 overflow-y-auto">
			{#each data.teams as team (team.value)}
				<label class="flex items-center gap-2 text-sm text-fg">
					<input
						type="checkbox"
						name="teamIds"
						value={team.value}
						checked={queue?.teamIds.includes(team.value) ?? false}
					/>
					Team: {team.label}
				</label>
			{/each}
			{#each data.people as person (person.value)}
				<label class="flex items-center gap-2 text-sm text-fg">
					<input
						type="checkbox"
						name="userIds"
						value={person.value}
						checked={queue?.userIds.includes(person.value) ?? false}
					/>
					{person.label}
				</label>
			{/each}
		</div>
	</fieldset>
{/snippet}

<div class="flex flex-col gap-6">
	<Card
		title="Service queues"
		description="Where tickets wait to be picked up. Members are the people and teams who work them."
	>
		{#if data.queues.length === 0}
			<p class="text-sm text-fg-muted">No queues yet.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border">
				{#each data.queues as queue (queue.id)}
					{@const names = [...queue.teamIds.map(teamName), ...queue.userIds.map(personName)]}
					<li class="flex flex-wrap items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
							<a
								href={resolve('/(app)/tickets') + `?view=open&queue=${queue.id}`}
								class="text-sm font-medium text-fg hover:underline"
							>
								{queue.name}
							</a>
							<!-- eslint-enable svelte/no-navigation-without-resolve -->
							<p class="mt-0.5 text-sm text-fg-muted">
								{queue.openTickets} open ticket{queue.openTickets === 1 ? '' : 's'} ·
								{names.length > 0 ? names.join(', ') : 'No members'}
							</p>
							{#if queue.description}
								<p class="mt-0.5 text-sm text-fg-muted">{queue.description}</p>
							{/if}
						</div>
						<div class="flex gap-2">
							<Button size="sm" onclick={() => edit(queue)}>Edit</Button>
							<form method="POST" action="?/archive" use:enhance={done('Queue archived.')}>
								<input type="hidden" name="id" value={queue.id} />
								<Button size="sm" type="submit" variant="ghost">Archive</Button>
							</form>
						</div>
					</li>
				{/each}
			</ul>
		{/if}
	</Card>

	<Card title="New queue">
		<form
			method="POST"
			action="?/create"
			class="flex flex-col gap-4"
			use:enhance={done('Queue created.')}
		>
			<div class="grid gap-4 sm:grid-cols-2">
				<Field
					label="Name"
					for="queue-name"
					class={data.providers.length > 1 ? '' : 'sm:col-span-2'}
				>
					<Input id="queue-name" name="name" required maxlength={100} placeholder="e.g. Helpdesk" />
				</Field>
				{#if data.providers.length > 1}
					<Field label="Belongs to" for="queue-org">
						<Select id="queue-org" name="orgId" bind:value={owner} options={data.providers} />
					</Field>
				{:else}
					<input type="hidden" name="orgId" value={owner} />
				{/if}
				<Field label="Description" for="queue-description" class="sm:col-span-2">
					<Input id="queue-description" name="description" maxlength={2000} />
				</Field>
			</div>
			{@render members(null)}
			<div class="flex justify-end">
				<Button type="submit" variant="primary">Create queue</Button>
			</div>
		</form>
	</Card>
</div>

<Dialog bind:open title="Edit queue">
	{#if editing}
		<form
			method="POST"
			action="?/update"
			class="flex flex-col gap-4"
			use:enhance={done('Queue saved.', () => (open = false))}
		>
			<input type="hidden" name="id" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
			<Field label="Name" for="edit-name">
				<Input id="edit-name" name="name" required maxlength={100} value={editing.name} />
			</Field>
			<Field label="Description" for="edit-description">
				<Input
					id="edit-description"
					name="description"
					maxlength={2000}
					value={editing.description}
				/>
			</Field>
			{@render members(editing)}
			<div class="flex justify-end gap-2">
				<Button onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</form>
	{/if}
</Dialog>
