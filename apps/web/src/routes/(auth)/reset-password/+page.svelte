<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Field, Input } from '@usehuxley/ui';
	import AuthShell from '#lib/AuthShell.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<AuthShell title="Choose a password">
	<form method="POST" class="flex flex-col gap-4" use:enhance>
		{#if form?.message}<p role="alert" class="text-sm text-danger">{form.message}</p>{/if}
		<input type="hidden" name="token" value={data.token} />
		<Field
			label="New password"
			for="newPassword"
			hint="At least 12 characters. A passphrase works well."
		>
			<Input
				id="newPassword"
				name="newPassword"
				type="password"
				autocomplete="new-password"
				minlength={12}
				required
			/>
		</Field>
		<Field label="Confirm password" for="confirmPassword">
			<Input
				id="confirmPassword"
				name="confirmPassword"
				type="password"
				autocomplete="new-password"
				required
			/>
		</Field>
		<Button type="submit" variant="primary" block>Set password</Button>
	</form>
	<p class="mt-4 text-center text-sm">
		<a href={resolve('/(auth)/forgot-password')} class="text-accent hover:underline">
			Need a new link?
		</a>
	</p>
</AuthShell>
