<script lang="ts">
	import { enhance } from '$app/forms';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { Button, Card, Field, Input, Select } from '@usehuxley/ui';
	import { LEVELS, SOURCES } from '#lib/tickets/labels.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let errors: Record<string, string> = $derived(form?.errors ?? {});
	let submitting = $state(false);
	let companyOptions = $derived(data.companies.map((c) => ({ value: c.orgId, label: c.name })));
	let assigneeOptions = $derived([
		{ value: '', label: 'Unassigned' },
		...data.people.map((p) => ({ value: `user:${p.value}`, label: p.label })),
		...data.teams.map((t) => ({ value: `team:${t.value}`, label: `Team: ${t.label}` }))
	]);

	/** This form with a client and template chosen (both kept in the URL). */
	function choose(org: string, template: string) {
		const query = [
			org ? `org=${encodeURIComponent(org)}` : '',
			template ? `template=${encodeURIComponent(template)}` : ''
		]
			.filter(Boolean)
			.join('&');
		// eslint-disable-next-line svelte/no-navigation-without-resolve -- resolve() plus a query string
		void goto(resolve('/(app)/tickets/new') + (query ? `?${query}` : ''), {
			replace: true,
			reset: false
		});
	}
	const chooseClient = (event: Event) =>
		choose((event.currentTarget as HTMLSelectElement).value, data.template?.id ?? '');
	const chooseTemplate = (event: Event) =>
		choose(data.orgId, (event.currentTarget as HTMLSelectElement).value);
	let t = $derived(data.template);
</script>

<svelte:head>
	<title>New ticket · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<div>
		<a href={resolve('/(app)/tickets')} class="text-sm text-fg-muted hover:text-fg">← Tickets</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">New ticket</h1>
	</div>

	<Card>
		<form
			method="POST"
			class="grid gap-4 sm:grid-cols-2"
			use:enhance={() => {
				submitting = true;
				return async ({ update }) => {
					await update({ reset: false });
					submitting = false;
				};
			}}
		>
			{#if form?.message}
				<p role="alert" class="text-sm text-danger sm:col-span-2">{form.message}</p>
			{/if}
			{#if data.templates.length > 0}
				<Field label="Template" for="template" class="sm:col-span-2">
					<Select
						id="template"
						value={data.template?.id ?? ''}
						options={[
							{ value: '', label: 'None' },
							...data.templates.map((tp) => ({ value: tp.id, label: tp.name }))
						]}
						onchange={chooseTemplate}
					/>
				</Field>
			{/if}
			<Field label="Client" for="orgId" error={errors.orgId} class="sm:col-span-2">
				<Select
					id="orgId"
					name="orgId"
					required
					value={data.orgId}
					placeholder="Choose a client"
					options={companyOptions}
					onchange={chooseClient}
				/>
			</Field>
			{#if data.orgId}
				{#key t?.id}
					<Field label="Title" for="title" error={errors.title} class="sm:col-span-2">
						<Input
							id="title"
							name="title"
							required
							maxlength={300}
							autofocus
							value={t?.title ?? ''}
						/>
					</Field>
					<Field
						label="Description"
						for="description"
						error={errors.description}
						class="sm:col-span-2"
					>
						<textarea
							id="description"
							name="description"
							rows="6"
							class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
							value={t?.description ?? ''}></textarea>
					</Field>
					<Field label="Requester" for="requesterId" error={errors.requesterId}>
						<Select
							id="requesterId"
							name="requesterId"
							value=""
							options={[
								{ value: '', label: 'None' },
								...data.contacts.map((c) => ({
									value: c.id,
									label: c.email ? `${c.name} (${c.email})` : c.name
								}))
							]}
						/>
					</Field>
					<Field label="Type" for="type" error={errors.type}>
						<Select
							id="type"
							name="type"
							value={t?.typeKey ?? 'incident'}
							options={data.types.map((t) => ({ value: t.key, label: t.name }))}
						/>
					</Field>
					<Field label="Impact" for="impact" error={errors.impact}>
						<Select
							id="impact"
							name="impact"
							value={String(t?.impact ?? 2)}
							options={[...LEVELS]}
						/>
					</Field>
					<Field label="Urgency" for="urgency" error={errors.urgency}>
						<Select
							id="urgency"
							name="urgency"
							value={String(t?.urgency ?? 2)}
							options={[...LEVELS]}
						/>
					</Field>
					<Field label="Queue" for="queueId" error={errors.queueId}>
						<Select
							id="queueId"
							name="queueId"
							value={t?.queueId ?? ''}
							options={[
								{ value: '', label: 'None' },
								...data.queues.map((q) => ({ value: q.id, label: q.name }))
							]}
						/>
					</Field>
					<Field
						label="Assignee"
						for="assignee"
						error={errors.assigneeUserId ?? errors.assigneeTeamId}
					>
						<Select id="assignee" name="assignee" value="" options={assigneeOptions} />
					</Field>
					<Field label="Received by" for="source" error={errors.source}>
						<Select id="source" name="source" value="web" options={[...SOURCES]} />
					</Field>
					<div class="flex justify-end gap-2 sm:col-span-2">
						<Button href={resolve('/(app)/tickets')}>Cancel</Button>
						<Button type="submit" variant="primary" disabled={submitting}>Create ticket</Button>
					</div>
				{/key}
			{/if}
		</form>
	</Card>
</div>
