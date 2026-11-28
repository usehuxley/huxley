<!-- Marks text an AI wrote (docs/security.md, prompt injection): what it
     read, such as an email or a ticket, can steer what it says, so it's a
     suggestion to check, not a fact. -->
<script lang="ts">
	import { Sparkles } from '@lucide/svelte';
	import { Badge } from '@usehuxley/ui';

	let { text = 'AI suggestion' }: { text?: string } = $props();
</script>

<span
	title="Written by AI from text that came from outside. Check it before acting on it."
	class="inline-flex"
>
	<Badge tone="info"><Sparkles size={12} aria-hidden="true" />{text}</Badge>
</span>
