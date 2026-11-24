<script lang="ts">
	import { enhance } from '$app/forms';
	import { invalidateAll } from '$app/navigation';
	import { onMount } from 'svelte';
	import { Badge, Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import { createPasskey, isCancelled, passkeysSupported, postJson } from '#lib/passkey.ts';
	import TokensCard from '#lib/tokens/TokensCard.svelte';
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

	let tokenForm = $derived(
		(form && 'tokens' in form ? form.tokens : {}) as {
			created?: { name: string; secret: string };
			error?: string;
		}
	);
	let passkeySupported = $state(false);
	let passkeyName = $state('Passkey');
	let passkeyPassword = $state('');
	let addingPasskey = $state(false);
	let passkeyError = $state<string | null>(null);
	/** Recovery codes issued with a first passkey, shown once. */
	let passkeyCodes = $state<string[] | null>(null);
	let shownCodes = $derived(passkeyCodes ?? mfaForm.recoveryCodes ?? null);

	onMount(() => {
		passkeySupported = passkeysSupported();
	});

	async function addPasskey(event: SubmitEvent) {
		event.preventDefault();
		addingPasskey = true;
		passkeyError = null;
		try {
			const start = await postJson<{ ceremony: string; options: never; message?: string }>(
				'/profile/passkeys/options',
				{ password: passkeyPassword }
			);
			if (!start.ok) {
				passkeyError = start.data.message ?? 'Could not start adding a passkey.';
				return;
			}
			const credential = await createPasskey(start.data.options);
			const finish = await postJson<{ recoveryCodes?: string[] | null; message?: string }>(
				'/profile/passkeys',
				{ ceremony: start.data.ceremony, name: passkeyName, credential }
			);
			if (!finish.ok) {
				passkeyError = finish.data.message ?? 'Could not add the passkey.';
				return;
			}
			passkeyCodes = finish.data.recoveryCodes ?? null;
			passkeyPassword = '';
			toast.push({ title: 'Passkey added', tone: 'success' });
			await invalidateAll();
		} catch (error) {
			if (!isCancelled(error)) passkeyError = "Your browser couldn't create a passkey.";
		} finally {
			addingPasskey = false;
		}
	}

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
			Manage your password, passkeys, two-factor authentication, API tokens, connected apps, and where you're signed
			in.
		</p>
	</div>

	{#if data.user.mfaSetupRequired}
		<div
			role="alert"
			class="rounded-card border border-warning/40 bg-warning-soft p-4 text-sm text-fg"
			data-testid="mfa-required"
		>
			<p class="font-medium">Two-factor authentication is required</p>
			<p class="mt-1 text-fg-muted">
				Your organization requires a second factor. Add a passkey or set up an authenticator app
				below to continue using Huxley.
			</p>
		</div>
	{/if}

	<Card title="Email" description={data.user.email}>
		{#snippet actions()}
			<Badge tone={data.user.emailVerified ? 'success' : 'warning'}>
				{data.user.emailVerified ? 'Verified' : 'Not verified'}
			</Badge>
		{/snippet}
		{#if form && 'verification' in form && form.verification && 'sent' in form.verification}
			<p role="status" class="text-sm text-fg">Check your inbox for a verification link.</p>
		{:else if !data.user.emailVerified && data.emailEnabled}
			<form method="POST" action="?/sendVerification" use:enhance>
				<Button type="submit" size="sm">Send verification email</Button>
			</form>
		{:else if !data.user.emailVerified}
			<p class="text-sm text-fg-muted">Email isn't set up on this server yet.</p>
		{:else}
			<p class="text-sm text-fg-muted">Password resets and notifications go here.</p>
		{/if}
	</Card>

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
		title="Passkeys"
		description="Sign in with your fingerprint, face, or device PIN instead of a password and code."
	>
		<div class="flex flex-col gap-4">
			{#if passkeyError}
				<p role="alert" class="text-sm text-danger">{passkeyError}</p>
			{/if}
			{#if data.passkeys.length > 0}
				<ul class="flex flex-col divide-y divide-border">
					{#each data.passkeys as passkey (passkey.id)}
						<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
							<div class="min-w-0">
								<p class="text-sm font-medium text-fg">{passkey.name}</p>
								<p class="mt-0.5 text-sm text-fg-muted">
									Added {formatDate(passkey.createdAt)} · Last used {formatDate(passkey.lastUsedAt)}
								</p>
							</div>
							<form method="POST" action="?/deletePasskey" use:enhance>
								<input type="hidden" name="id" value={passkey.id} />
								<Button type="submit" size="sm" aria-label="Remove {passkey.name}">Remove</Button>
							</form>
						</li>
					{/each}
				</ul>
			{/if}
			{#if !data.passkeysEnabled}
				<p class="text-sm text-fg-muted">
					Passkeys aren't available: an administrator needs to set this server's public URL.
				</p>
			{:else if !passkeySupported}
				<p class="text-sm text-fg-muted">This browser doesn't support passkeys.</p>
			{:else}
				<form class="flex flex-wrap items-end gap-2" onsubmit={addPasskey}>
					<Field label="Name" for="passkeyName">
						<Input id="passkeyName" name="name" bind:value={passkeyName} maxlength={100} required />
					</Field>
					<Field label="Password" for="passkeyPassword">
						<Input
							id="passkeyPassword"
							name="password"
							type="password"
							autocomplete="current-password"
							bind:value={passkeyPassword}
							required
						/>
					</Field>
					<Button type="submit" variant="primary" disabled={addingPasskey}>
						{addingPasskey ? 'Waiting for your device…' : 'Add a passkey'}
					</Button>
				</form>
			{/if}
		</div>
	</Card>

	<Card
		title="Authenticator app"
		description="Enter a six-digit code from an app when you sign in."
	>
		<div class="flex flex-col gap-4">
			{#if mfaForm.error}
				<p role="alert" class="text-sm text-danger">{mfaForm.error}</p>
			{/if}

			{#if data.mfa.totpEnabled}
				<p class="flex items-center gap-2 text-sm text-fg">
					<Badge tone="success">On</Badge> Codes from your authenticator app are accepted.
				</p>
				<div>
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
				<p class="flex items-center gap-2 text-sm text-fg-muted"><Badge>Off</Badge> Not set up.</p>
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

	{#if shownCodes || data.mfa.totpEnabled || data.mfa.passkeys > 0}
		<Card
			title="Recovery codes"
			description="Each code signs you in once if you lose your passkeys and authenticator."
		>
			<div class="flex flex-col gap-4">
				{#if shownCodes}
					<div class="rounded-control border border-warning/40 bg-warning-soft p-4">
						<p class="text-sm font-medium text-fg">Save your recovery codes</p>
						<p class="mt-1 text-sm text-fg-muted">
							They're shown only now: store them in your password manager.
						</p>
						<ul class="mt-3 grid grid-cols-2 gap-x-6 gap-y-1 font-mono text-sm text-fg">
							{#each shownCodes as code (code)}
								<li>{code}</li>
							{/each}
						</ul>
					</div>
				{/if}
				<p class="text-sm text-fg">
					{data.mfa.recoveryCodesRemaining} recovery code{data.mfa.recoveryCodesRemaining === 1
						? ''
						: 's'} left
				</p>
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
			</div>
		</Card>
	{/if}

	{#if !data.user.mfaSetupRequired}
		<TokensCard
			tokens={data.tokens}
			permissions={data.permissions}
			created={tokenForm.created ?? null}
			error={tokenForm.error ?? null}
			description="Personal access tokens act as you from scripts and tools, limited to what you choose. Each expires within a year."
		/>
	{/if}

	<Card
		title="Connected apps"
		description="Apps you approved to act as you, such as AI clients using Huxley's MCP server. Disconnecting one stops it at once."
	>
		{#if data.connectedApps.length === 0}
			<p class="text-sm text-fg-muted">No apps are connected.</p>
		{:else}
			<ul class="flex flex-col divide-y divide-border" aria-label="Connected apps">
				{#each data.connectedApps as app (app.id)}
					<li class="flex items-center justify-between gap-4 py-3 first:pt-0 last:pb-0">
						<div class="min-w-0">
							<p class="flex items-center gap-2 text-sm font-medium text-fg">
								{app.name}
								<Badge>{app.scopes.includes('*') ? 'Full access' : `${app.scopes.length} permissions`}</Badge>
							</p>
							<p class="mt-0.5 text-sm text-fg-muted">
								Connected {formatDate(app.connectedAt)} · {app.lastUsedAt
									? `Last used ${formatDate(app.lastUsedAt)}${app.lastUsedIp ? ` from ${app.lastUsedIp}` : ''}`
									: 'Not used yet'}
							</p>
						</div>
						<form method="POST" action="?/disconnectApp" use:enhance>
							<input type="hidden" name="id" value={app.id} />
							<Button type="submit" size="sm">Disconnect</Button>
						</form>
					</li>
				{/each}
			</ul>
		{/if}
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
