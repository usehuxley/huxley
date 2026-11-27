<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();
	let r = $derived(data.retention);
	const said: SubmitFunction =
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			const back = result.type === 'success' || result.type === 'failure' ? result.data : undefined;
			const message = (back as { message?: string } | undefined)?.message;
			toast.push({
				title: message ?? 'Done.',
				tone: result.type === 'success' ? 'success' : 'danger'
			});
		};
</script>

<svelte:head>
	<title>Data retention · Settings · Huxley</title>
</svelte:head>

{#if r}
	<div class="flex flex-col gap-6">
		<Card
			title="Data retention"
			description="How long Huxley keeps what grows without end. A daily sweep removes what's older. The audit log is kept: export it instead."
		>
			<form
				method="POST"
				action="?/save"
				class="grid max-w-2xl gap-4 sm:grid-cols-2"
				use:enhance={said}
			>
				<Field
					label="Finished workflow runs (months)"
					for="run-months"
					hint="With their step details and stored payloads"
				>
					<Input
						id="run-months"
						name="runMonths"
						type="number"
						min={1}
						max={120}
						value={r.runMonths}
					/>
				</Field>
				<Field label="Delivered events (days)" for="event-days">
					<Input
						id="event-days"
						name="eventDays"
						type="number"
						min={1}
						max={3650}
						value={r.eventDays}
					/>
				</Field>
				<Field label="Read notifications (days)" for="notification-days">
					<Input
						id="notification-days"
						name="notificationDays"
						type="number"
						min={1}
						max={3650}
						value={r.notificationDays}
					/>
				</Field>
				<Field label="AI call records (days)" for="ai-days" hint="Usage and cost history">
					<Input
						id="ai-days"
						name="aiCallDays"
						type="number"
						min={1}
						max={3650}
						value={r.aiCallDays}
					/>
				</Field>
				<div class="sm:col-span-2"><Button type="submit" variant="primary">Save</Button></div>
			</form>
		</Card>
		<Card title="The sweep">
			<div class="flex flex-wrap items-center justify-between gap-3 text-sm">
				<p class="text-fg-muted">
					{#if r.lastSweptAt && r.lastSweep}
						Last ran {new Date(r.lastSweptAt).toLocaleString()}: removed {r.lastSweep.runs} runs ({r
							.lastSweep.payloadObjects}
						stored payloads), {r.lastSweep.events} events, {r.lastSweep.notifications} notifications and
						{r.lastSweep.aiCalls} AI call records.
					{:else}
						It hasn't run yet.
					{/if}
				</p>
				<form method="POST" action="?/sweep" use:enhance={said}>
					<Button type="submit">Sweep now</Button>
				</form>
			</div>
		</Card>
	</div>
{/if}
