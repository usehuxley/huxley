<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Field, Input } from '@usehuxley/ui';
	import AuthShell from '#lib/AuthShell.svelte';
	import type { PageProps } from './$types';

	let { form }: PageProps = $props();
</script>

<AuthShell title="Reset your password">
	{#if form?.sent}
		<p role="status" class="text-sm text-fg">
			If an account with a password uses that address, a reset link is on its way. It works for 30
			minutes.
		</p>
	{:else}
		<form method="POST" class="flex flex-col gap-4" use:enhance>
			{#if form?.message}<p role="alert" class="text-sm text-danger">{form.message}</p>{/if}
			<Field label="Email" for="email">
				<Input id="email" name="email" type="email" autocomplete="username" required />
			</Field>
			<Button type="submit" variant="primary" block>Send reset link</Button>
		</form>
	{/if}
	<p class="mt-4 text-center text-sm">
		<a href={resolve('/(auth)/login')} class="text-accent hover:underline">Back to sign in</a>
	</p>
</AuthShell>
