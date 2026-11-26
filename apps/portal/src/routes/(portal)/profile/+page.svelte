<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Card, Field, Input } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
</script>

<svelte:head>
	<title>Profile · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<h1 class="text-2xl font-semibold text-fg">Your profile</h1>
	<Card title={data.me.name}>
		<p class="text-sm text-fg-muted">{data.me.email} · {data.me.orgName}</p>
	</Card>
	<Card
		title="Password"
		description="Optional: sign in with a password instead of an emailed link. At least 12 characters."
	>
		<form method="POST" action="?/password" class="flex flex-col gap-3" use:enhance>
			<Field label="New password" for="password">
				<Input id="password" name="password" type="password" autocomplete="new-password" required />
			</Field>
			<Field label="Again" for="again">
				<Input id="again" name="again" type="password" autocomplete="new-password" required />
			</Field>
			<div><Button type="submit">Set password</Button></div>
		</form>
		{#if form && 'done' in form}<p class="mt-3 text-sm text-success" role="status">
				Your password is set. Your other sessions were signed out.
			</p>{/if}
		{#if form && 'message' in form}<p class="mt-3 text-sm text-danger" role="alert">
				{form.message}
			</p>{/if}
	</Card>
</div>
