<!-- Where a file stands with the malware scan (ADR-094). Nothing for files
     scanned clean or uploaded with scanning off. -->
<script lang="ts">
	import { Badge } from '@usehuxley/ui';

	let { status }: { status: string | null | undefined } = $props();
</script>

{#if status === 'pending'}
	<Badge>Scanning…</Badge>
{:else if status === 'infected'}
	<Badge tone="danger">Quarantined</Badge>
{:else if status === 'failed'}
	<Badge tone="warning">Not scanned</Badge>
{/if}
