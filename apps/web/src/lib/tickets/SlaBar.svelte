<script lang="ts">
	import { clockTone, clockUsed } from '@usehuxley/ui';

	/**
	 * How much of a ticket's resolution SLA has gone, as a short bar and the
	 * time left. Empty when the ticket has no running clock.
	 */
	let {
		createdAt,
		dueAt,
		paused = false,
		now
	}: { createdAt: string; dueAt?: string | null; paused?: boolean; now: number } = $props();

	let used = $derived(dueAt ? clockUsed({ from: createdAt, due: dueAt }, now) : 0);
	let tone = $derived(clockTone(used));
	let left = $derived(dueAt ? new Date(dueAt).getTime() - now : 0);
	let text = $derived.by(() => {
		if (paused) return 'paused';
		const mins = Math.round(Math.abs(left) / 60_000);
		const span =
			mins >= 1440
				? `${Math.round(mins / 1440)}d`
				: `${Math.floor(mins / 60)}:${String(mins % 60).padStart(2, '0')}`;
		return left < 0 ? `−${span}` : span;
	});
</script>

{#if dueAt}
	<span
		class="flex min-w-32 items-center gap-2"
		title={paused
			? 'SLA paused'
			: left < 0
				? `Resolution SLA missed by ${text.slice(1)}`
				: `${text} until the resolution SLA`}
	>
		<span class="h-1.5 flex-1 overflow-hidden rounded-full bg-surface-sunken" aria-hidden="true">
			<span
				class="block h-full rounded-full {paused
					? 'bg-fg-subtle'
					: tone === 'danger'
						? 'bg-danger'
						: tone === 'warn'
							? 'bg-warning'
							: 'bg-success'}"
				style="width:{Math.min(100, used * 100)}%"
			></span>
		</span>
		<span
			class="w-12 text-right font-mono text-[12.5px] font-semibold tabular-nums {left < 0 && !paused
				? 'text-danger'
				: 'text-fg'}"
		>
			{text}
		</span>
	</span>
{/if}
