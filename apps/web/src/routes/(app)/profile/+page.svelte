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

	type MfaForm = {
		error?: string;
		enrollment?: { secret: string; otpauthUri: string; qr: string };
		recoveryCodes?: string[];
		enabled?: boolean;
		disabled?: boolean;
	};
	let mfaForm = $derived((form && 'mfa' in form ? form.mfa : {}) as MfaForm);
	/** Groups the base32 secret in fours so it's easier to type. */
	const grouped = (secret: string) => secret.replace(/(.{4})/g, '$1 ').trim();

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
		<p class="mt-1 text-fg-muted">
			Manage your password, two-factor authentication, and where you're signed in.
		</p>
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

	<Card
		title="Two-factor authentication"
		description="Ask for a code from an authenticator app when you sign in."
	>
		<div class="flex flex-col gap-4">
			{#if mfaForm.error}
				<p role="alert" class="text-sm text-danger">{mfaForm.error}</p>
			{/if}

			{#if mfaForm.recoveryCodes}
				<div class="rounded-control border border-warning/40 bg-warning-soft p-4">
					<p class="text-sm font-medium text-fg">Save your recovery codes</p>
					<p class="mt-1 text-sm text-fg-muted">
						If you lose your authenticator, each code signs you in once. They're shown only now:
						store them in your password manager.
					</p>
					<ul class="mt-3 grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-sm text-fg">
						{#each mfaForm.recoveryCodes as code (code)}
							<li>{code}</li>
						{/each}
					</ul>
				</div>
			{/if}

			{#if data.mfa.totpEnabled}
				<p class="flex items-center gap-2 text-sm text-fg">
					<Badge tone="success">On</Badge>
					Authenticator app · {data.mfa.recoveryCodesRemaining} recovery code{data.mfa
						.recoveryCodesRemaining === 1
						? ''
						: 's'} left
				</p>
				<div class="flex flex-col gap-4 sm:flex-row sm:items-end">
					<form method="POST" action="?/regenerateCodes" class="flex items-end gap-2" use:enhance>
						<Field label="Password" for="regenPassword">
							<Input
								id="regenPassword"
								name="password"
								type="password"
								autocomplete="current-password"
								required
							/>
						</Field>
						<Button type="submit">New recovery codes</Button>
					</form>
					<form
						method="POST"
						action="?/disableTotp"
						class="flex items-end gap-2"
						use:enhance={() =>
							async ({ result, update }) => {
								await update();
								if (result.type === 'success') {
									toast.push({ title: 'Two-factor authentication turned off' });
								}
							}}
					>
						<Field label="Password" for="disablePassword">
							<Input
								id="disablePassword"
								name="password"
								type="password"
								autocomplete="current-password"
								required
							/>
						</Field>
						<Button type="submit" variant="danger">Turn off</Button>
					</form>
				</div>
			{:else if mfaForm.enrollment}
				<ol class="flex flex-col gap-4 text-sm text-fg">
					<li>
						<p>
							1. Scan this code with your authenticator app (Microsoft Authenticator, 1Password, …).
						</p>
						<!-- Generated on our server from the otpauth URI; contains no user-controlled markup. -->
						<div class="mt-3 size-44 rounded-control bg-white p-2" aria-hidden="true">
							<!-- eslint-disable-next-line svelte/no-at-html-tags -->
							{@html mfaForm.enrollment.qr}
						</div>
						<p class="mt-2 text-fg-muted">
							Can't scan it? Enter this key instead:
							<code class="font-mono text-fg" data-testid="totp-secret">
								{grouped(mfaForm.enrollment.secret)}
							</code>
						</p>
					</li>
					<li>
						<p>2. Enter the six-digit code the app shows.</p>
						<form
							method="POST"
							action="?/confirmTotp"
							class="mt-3 flex items-end gap-2"
							use:enhance
						>
							<input type="hidden" name="secret" value={mfaForm.enrollment.secret} />
							<input type="hidden" name="otpauthUri" value={mfaForm.enrollment.otpauthUri} />
							<Field label="Code" for="totpCode">
								<Input
									id="totpCode"
									name="code"
									inputmode="numeric"
									autocomplete="one-time-code"
									maxlength={7}
									required
								/>
							</Field>
							<Button type="submit" variant="primary">Turn on</Button>
						</form>
					</li>
				</ol>
			{:else}
				<p class="flex items-center gap-2 text-sm text-fg-muted">
					<Badge>Off</Badge> Only your password protects your account.
				</p>
				<form method="POST" action="?/startTotp" class="flex items-end gap-2" use:enhance>
					<Field label="Password" for="totpPassword">
						<Input
							id="totpPassword"
							name="password"
							type="password"
							autocomplete="current-password"
							required
						/>
					</Field>
					<Button type="submit" variant="primary">Set up authenticator app</Button>
				</form>
			{/if}
		</div>
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
