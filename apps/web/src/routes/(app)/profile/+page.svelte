<script lang="ts">
	import { enhance } from '$app/forms';
	import { Badge, Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();

	let passwordErrors = $derived(
		(form?.password && 'errors' in form.password ? form.password.errors : {}) as Record<
			string,
			string
		>
	);

	const dateFormat = new Intl.DateTimeFormat(undefined, {
		dateStyle: 'medium',
		timeStyle: 'short'
	});
	const formatDate = (iso: string | null | undefined) =>
		iso ? dateFormat.format(new Date(iso)) : '—';

	/** A short, readable label for a user agent string. */
	function device(userAgent: string | null | undefined): string {
		if (!userAgent) return 'Unknown device';
		const browser = /Edg\//.test(userAgent)
			? 'Edge'
			: /Firefox\//.test(userAgent)
				? 'Firefox'
				: /Chrome\//.test(userAgent)
					? 'Chrome'
					: /Safari\//.test(userAgent)
						? 'Safari'
						: 'Browser';
		const os = /Windows/.test(userAgent)
			? 'Windows'
			: /Mac OS X/.test(userAgent)
				? 'macOS'
				: /Android/.test(userAgent)
					? 'Android'
					: /iPhone|iPad/.test(userAgent)
						? 'iOS'
						: /Linux/.test(userAgent)
							? 'Linux'
							: '';
		return os ? `${browser} on ${os}` : browser;
	}
</script>

<svelte:head>
	<title>Profile · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-3xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Your profile</h1>
		<p class="mt-1 text-fg-muted">Manage your password and where you're signed in.</p>
	</div>

	<Card title="Change password" description="Changing it signs you out everywhere else.">
		<form
			method="POST"
			action="?/changePassword"
			class="flex max-w-sm flex-col gap-4"
			use:enhance={() =>
				async ({ result, update }) => {
					await update();
					if (result.type === 'success') {
						toast.push({ title: 'Password changed', tone: 'success' });
					}
				}}
		>
			{#if passwordErrors.form}
				<p role="alert" class="text-sm text-danger">{passwordErrors.form}</p>
			{/if}
			<Field label="Current password" for="currentPassword" error={passwordErrors.currentPassword}>
				<Input
					id="currentPassword"
					name="currentPassword"
					type="password"
					autocomplete="current-password"
					required
					invalid={!!passwordErrors.currentPassword}
				/>
			</Field>
			<Field
				label="New password"
				for="newPassword"
				hint="At least 12 characters. A passphrase works well."
				error={passwordErrors.newPassword}
			>
				<Input
					id="newPassword"
					name="newPassword"
					type="password"
					autocomplete="new-password"
					minlength={12}
					required
					invalid={!!passwordErrors.newPassword}
				/>
			</Field>
			<Field
				label="Confirm new password"
				for="confirmPassword"
				error={passwordErrors.confirmPassword}
			>
				<Input
					id="confirmPassword"
					name="confirmPassword"
					type="password"
					autocomplete="new-password"
					required
					invalid={!!passwordErrors.confirmPassword}
				/>
			</Field>
			<div><Button type="submit" variant="primary">Change password</Button></div>
		</form>
	</Card>

	<Card title="Active sessions" description="Devices currently signed in to your account.">
		<ul class="flex flex-col divide-y divide-border">
			{#each data.sessions as session (session.id)}
				<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
					<div class="min-w-0">
						<p class="flex items-center gap-2 text-sm font-medium text-fg">
							{device(session.userAgent)}
							{#if session.current}<Badge tone="accent">This device</Badge>{/if}
						</p>
						<p class="mt-0.5 text-sm text-fg-muted">
							{session.ip ?? 'Unknown address'} · Last active {formatDate(
								session.lastSeenAt ?? session.createdAt
							)}
						</p>
					</div>
					{#if !session.current}
						<form method="POST" action="?/revokeSession" use:enhance>
							<input type="hidden" name="id" value={session.id} />
							<Button type="submit" size="sm">Sign out</Button>
						</form>
					{/if}
				</li>
			{/each}
		</ul>
	</Card>
</div>
