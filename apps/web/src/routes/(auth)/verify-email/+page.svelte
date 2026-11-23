<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button } from '@usehuxley/ui';
	import AuthShell from '#lib/AuthShell.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<AuthShell title="Verify your email">
	{#if form?.verified}
		<p role="status" class="text-sm text-fg">Your email address is verified.</p>
	{:else}
		<form method="POST" class="flex flex-col gap-4" use:enhance>
			{#if form?.message}<p role="alert" class="text-sm text-danger">{form.message}</p>{/if}
			<input type="hidden" name="token" value={data.token} />
			<p class="text-sm text-fg-muted">Confirm that this is your email address.</p>
			<Button type="submit" variant="primary" block>Verify email</Button>
		</form>
	{/if}
	<p class="mt-4 text-center text-sm">
		<a href={resolve('/(app)')} class="text-accent hover:underline">Go to Huxley</a>
	</p>
</AuthShell>
