<script lang="ts">
	import { Field, Input, Select } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';
	import { KINDS, SECURITY, UNKNOWN_SENDER } from './form.ts';

	type Mailbox = components['schemas']['Mailbox'];
	type Option = { value: string; label: string };

	let {
		mailbox = null,
		errors = {},
		types,
		queues,
		orgs
	}: {
		/** The mailbox to edit, or none for a new one. */
		mailbox?: Mailbox | null;
		errors?: Record<string, string>;
		types: Option[];
		queues: Option[];
		/** Organizations unknown senders' tickets can go to. */
		orgs: Option[];
	} = $props();

	// svelte-ignore state_referenced_locally
	let kind = $state(mailbox?.kind ?? 'imap');
	// svelte-ignore state_referenced_locally
	let security = $state(mailbox?.imap?.security ?? 'tls');
</script>

{#if mailbox}
	<input type="hidden" name="version" value={mailbox.version} />
{/if}
<div class="grid gap-4 sm:grid-cols-2">
	<Field label="Name" for="mb-name" error={errors.name}>
		<Input
			id="mb-name"
			name="name"
			required
			maxlength={100}
			value={mailbox?.name ?? ''}
			placeholder="e.g. Support"
		/>
	</Field>
	<Field label="Address" for="mb-address" error={errors.address}>
		<Input
			id="mb-address"
			name="address"
			type="email"
			required
			value={mailbox?.address ?? ''}
			placeholder="support@your-msp.example"
		/>
	</Field>
	<Field
		label="How mail arrives"
		for="mb-kind"
		error={errors.kind}
		class="sm:col-span-2"
		hint={kind === 'forward'
			? 'Your mail server or a mail service posts each message to Huxley.'
			: 'Huxley fetches new mail every minute (with the Service desk email pack).'}
	>
		<Select
			id="mb-kind"
			name="kind"
			bind:value={kind}
			options={[
				...KINDS,
				...(kind === 'graph' ? [{ value: 'graph', label: 'Microsoft 365' }] : [])
			]}
		/>
	</Field>

	{#if kind === 'imap'}
		<Field label="IMAP server" for="mb-host" error={errors.imap}>
			<Input
				id="mb-host"
				name="host"
				required
				value={mailbox?.imap?.host ?? ''}
				placeholder="imap.example.com"
			/>
		</Field>
		<Field label="Security" for="mb-security">
			<Select id="mb-security" name="security" bind:value={security} options={[...SECURITY]} />
		</Field>
		<Field label="Port" for="mb-port" hint="Empty: the usual one for the security chosen.">
			<Input
				id="mb-port"
				name="port"
				type="number"
				min={1}
				max={65535}
				value={mailbox?.imap?.port ? String(mailbox.imap.port) : ''}
			/>
		</Field>
		<Field label="Folder" for="mb-folder">
			<Input id="mb-folder" name="folder" value={mailbox?.imap?.folder ?? 'INBOX'} />
		</Field>
		<Field label="Username" for="mb-username">
			<Input
				id="mb-username"
				name="username"
				required
				autocomplete="off"
				value={mailbox?.imap?.username ?? ''}
			/>
		</Field>
		<Field
			label="Password"
			for="mb-password"
			error={errors.password}
			hint={mailbox?.hasPassword
				? 'Saved. Leave empty to keep it.'
				: 'Kept encrypted; never shown again.'}
		>
			<Input id="mb-password" name="password" type="password" autocomplete="new-password" />
		</Field>
	{/if}

	<Field label="New tickets are" for="mb-type" error={errors.ticketType}>
		<Select
			id="mb-type"
			name="ticketType"
			value={mailbox?.ticketType ?? 'incident'}
			options={types}
		/>
	</Field>
	<Field label="In queue" for="mb-queue" error={errors.queueId}>
		<Select
			id="mb-queue"
			name="queueId"
			value={mailbox?.queueId ?? ''}
			options={[{ value: '', label: 'None' }, ...queues]}
		/>
	</Field>
	<Field
		label="When the sender isn’t a contact"
		for="mb-unknown"
		error={errors.unknownSender}
		hint="A sender whose domain matches a company's contacts goes to that company."
	>
		<Select
			id="mb-unknown"
			name="unknownSender"
			value={mailbox?.unknownSender ?? 'create_contact'}
			options={[...UNKNOWN_SENDER]}
		/>
	</Field>
	<Field label="Otherwise, at" for="mb-unknown-org" error={errors.unknownOrgId}>
		<Select
			id="mb-unknown-org"
			name="unknownOrgId"
			value={mailbox?.unknownOrgId ?? ''}
			options={[{ value: '', label: 'The mailbox’s organization' }, ...orgs]}
		/>
	</Field>
	<label class="flex items-center gap-2 text-sm text-fg">
		<input type="checkbox" name="stripReplies" checked={mailbox?.stripReplies ?? true} />
		Strip quoted replies and signatures
	</label>
	<label class="flex items-center gap-2 text-sm text-fg">
		<input type="checkbox" name="enabled" checked={mailbox?.enabled ?? true} />
		Taking mail
	</label>
</div>
