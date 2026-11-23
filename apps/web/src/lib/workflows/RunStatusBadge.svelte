<script lang="ts">
	import { Badge, type BadgeTone } from '@usehuxley/ui';

	let { status }: { status: string } = $props();
	const tones: Record<string, BadgeTone> = {
		queued: 'neutral',
		running: 'info',
		waiting: 'info',
		succeeded: 'success',
		failed: 'danger',
		cancelled: 'warning',
		skipped: 'neutral'
	};
	const labels: Record<string, string> = {
		queued: 'Queued',
		running: 'Running',
		waiting: 'Waiting',
		succeeded: 'Succeeded',
		failed: 'Failed',
		cancelled: 'Cancelled',
		skipped: 'Skipped'
	};
</script>

<Badge tone={tones[status] ?? 'neutral'}>{labels[status] ?? status}</Badge>
