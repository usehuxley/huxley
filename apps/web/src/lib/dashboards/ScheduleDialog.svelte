<!-- Email a dashboard on a schedule: made as a workflow. -->
<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Dialog, Field, Select } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let {
		open = $bindable(false),
		me,
		teams,
		problem = null,
		scheduled = null
	}: {
		open?: boolean;
		me: string;
		teams: components['schemas']['AudienceOption'][];
		problem?: string | null;
		scheduled?: string | null;
	} = $props();

	let recipients = $derived([
		{ value: `user:${me}`, label: 'Me' },
		{ value: 'role:admin', label: 'Administrators' },
		...teams.map((t) => ({ value: `team:${t.id}`, label: `Team: ${t.name}` }))
	]);
	const HOURS = Array.from({ length: 24 }, (_, h) => ({
		value: String(h),
		label: `${String(h).padStart(2, '0')}:00`
	}));
</script>

<Dialog bind:open title="Email this regularly">
	{#if scheduled}
		<p class="text-sm text-fg">
			Scheduled. It runs as a workflow:
			<a
				href={resolve('/(app)/workflows/[id]', { id: scheduled })}
				class="text-accent hover:underline"
			>
				see or change it
			</a>.
		</p>
	{:else}
		<form method="POST" action="?/schedule" class="flex flex-col gap-4" use:enhance>
			<p class="text-sm text-fg-muted">
				People get it by email, with the numbers of your current organization and those below. It's
				made as a workflow you can change or pause.
			</p>
			<div class="grid gap-3 sm:grid-cols-2">
				<Field label="How often" for="sched-cadence">
					<Select
						id="sched-cadence"
						name="cadence"
						value="weekly"
						options={[
							{ value: 'daily', label: 'Every day' },
							{ value: 'weekly', label: 'Every Monday' },
							{ value: 'monthly', label: 'On the 1st of the month' }
						]}
					/>
				</Field>
				<Field label="At" for="sched-hour">
					<Select id="sched-hour" name="hour" value="8" options={HOURS} />
				</Field>
				<Field label="As" for="sched-format">
					<Select
						id="sched-format"
						name="format"
						value="pdf"
						options={[
							{ value: 'pdf', label: 'PDF' },
							{ value: 'xlsx', label: 'Excel (XLSX)' },
							{ value: 'csv', label: 'CSV' }
						]}
					/>
				</Field>
				<Field label="To" for="sched-to">
					<Select id="sched-to" name="to" options={recipients} />
				</Field>
			</div>
			{#if problem}<p class="text-sm text-danger" role="alert">{problem}</p>{/if}
			<div class="flex justify-end gap-2">
				<Button type="button" variant="ghost" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Schedule</Button>
			</div>
		</form>
	{/if}
</Dialog>
