<script lang="ts">
	import { Input } from '@usehuxley/ui';
	import { DAYS } from './labels.ts';

	/**
	 * Opening hours, one range a day (more can be set through the API).
	 * Posts `open_<day>`, `from_<day>` and `to_<day>`; read them back with
	 * `hoursFromForm`.
	 */
	let {
		hours = null,
		label = 'Opening hours',
		error
	}: {
		/** Hours to start from; none: weekdays 09:00 to 17:00. */
		hours?: Record<string, string[][]> | null;
		label?: string;
		error?: string;
	} = $props();

	const range = (day: string) => hours?.[day]?.[0] ?? null;
</script>

<fieldset>
	<legend class="text-sm font-medium text-fg">{label}</legend>
	{#if error}<p class="text-sm text-danger">{error}</p>{/if}
	<div class="mt-2 flex flex-col gap-1.5">
		{#each DAYS as day (day.key)}
			{@const open = range(day.key)}
			<div class="grid grid-cols-[7rem_1fr_1fr] items-center gap-2 text-sm">
				<label class="flex items-center gap-2 text-fg">
					<input
						type="checkbox"
						name="open_{day.key}"
						checked={hours ? open !== null : !['sat', 'sun'].includes(day.key)}
					/>
					{day.label}
				</label>
				<Input
					type="time"
					name="from_{day.key}"
					value={open?.[0] ?? '09:00'}
					aria-label="{day.label} opens"
				/>
				<Input
					type="time"
					name="to_{day.key}"
					value={open?.[1] ?? '17:00'}
					aria-label="{day.label} closes"
				/>
			</div>
		{/each}
	</div>
</fieldset>
