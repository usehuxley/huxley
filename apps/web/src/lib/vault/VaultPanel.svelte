<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { Copy, Eye, EyeOff, KeyRound, Plus } from '@lucide/svelte';
	import {
		Badge,
		Button,
		Card,
		Dialog,
		EmptyState,
		Field,
		Input,
		Select,
		toast
	} from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	type Item = components['schemas']['VaultItem'];
	let { items, canManage }: { items: Item[]; canManage: boolean } = $props();

	const KINDS = [
		{ value: 'login', label: 'Login' },
		{ value: 'api_key', label: 'API key' },
		{ value: 'wifi', label: 'Wi-Fi' },
		{ value: 'note', label: 'Secret note' },
		{ value: 'other', label: 'Other' }
	];
	const kindLabel = (k: string) => KINDS.find((x) => x.value === k)?.label ?? k;

	let editing: Item | null = $state(null);
	let open = $state(false);
	let errors: Record<string, string> = $state({});

	// The one secret on screen: hidden again after 30 seconds.
	let shown: { id: string; secret: string } | null = $state(null);
	let hideTimer: ReturnType<typeof setTimeout> | undefined;
	let revealing: Item | null = $state(null);
	let revealOpen = $state(false);
	let reason = $state('');

	function hide() {
		clearTimeout(hideTimer);
		shown = null;
	}

	async function reveal() {
		const item = revealing;
		if (!item) return;
		const res = await fetch(`/vault/${item.id}/reveal`, {
			method: 'POST',
			headers: { 'content-type': 'application/json' },
			body: JSON.stringify({ reason })
		});
		const body = (await res.json()) as { secret?: string; message?: string };
		revealOpen = false;
		revealing = null;
		reason = '';
		if (!res.ok || body.secret === undefined) {
			toast.push({ title: body.message ?? 'Could not reveal it.', tone: 'danger' });
			return;
		}
		clearTimeout(hideTimer);
		shown = { id: item.id, secret: body.secret };
		hideTimer = setTimeout(hide, 30_000);
	}

	async function copy(text: string, what: string) {
		await navigator.clipboard.writeText(text);
		toast.push({ title: `${what} copied.`, tone: 'success' });
	}

	const done =
		(message: string): SubmitFunction =>
		() =>
		async ({ result, update }) => {
			await update({ reset: false });
			if (result.type === 'success') {
				toast.push({ title: message, tone: 'success' });
				open = false;
				errors = {};
			} else if (result.type === 'failure') {
				const failed = (
					result.data as { vault?: { message?: string; errors?: Record<string, string> } }
				)?.vault;
				errors = failed?.errors ?? {};
				toast.push({ title: failed?.message ?? 'Something went wrong.', tone: 'danger' });
			}
		};
	const isLink = (url: string) => /^https?:\/\//i.test(url);
	const when = (v: string) => new Date(v).toLocaleString();
</script>

<Card title="Passwords" description="Encrypted. Every reveal is recorded with who, when and why.">
	{#snippet actions()}
		{#if canManage}
			<Button
				onclick={() => {
					editing = null;
					errors = {};
					open = true;
				}}
			>
				<Plus size={16} /> Add
			</Button>
		{/if}
	{/snippet}
	{#if items.length === 0}
		<EmptyState
			title="No passwords yet"
			description="Keep this client's admin logins, keys and Wi-Fi passwords here."
		>
			{#snippet icon()}<KeyRound size={20} />{/snippet}
		</EmptyState>
	{:else}
		<ul class="flex flex-col divide-y divide-border" aria-label="Passwords">
			{#each items as item (item.id)}
				<li class="flex flex-col gap-1.5 py-3 first:pt-0 last:pb-0">
					<div class="flex flex-wrap items-center justify-between gap-2">
						<div class="flex min-w-0 flex-wrap items-center gap-2">
							<span class="font-medium text-fg">{item.name}</span>
							<Badge>{kindLabel(item.kind)}</Badge>
							{#each item.tags as tag (tag)}<Badge tone="info">{tag}</Badge>{/each}
						</div>
						<div class="flex shrink-0 gap-2">
							{#if item.hasSecret}
								{#if shown?.id === item.id}
									<Button
										size="sm"
										variant="ghost"
										onclick={hide}
										aria-label="Hide the secret of {item.name}"
									>
										<EyeOff size={14} /> Hide
									</Button>
								{:else}
									<Button
										size="sm"
										onclick={() => {
											revealing = item;
											revealOpen = true;
										}}
										aria-label="Reveal the secret of {item.name}"
									>
										<Eye size={14} /> Reveal
									</Button>
								{/if}
							{/if}
							{#if canManage}
								<Button
									size="sm"
									variant="ghost"
									onclick={() => {
										editing = item;
										errors = {};
										open = true;
									}}
									aria-label="Edit {item.name}"
								>
									Edit
								</Button>
							{/if}
						</div>
					</div>
					<div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-sm text-fg-muted">
						{#if item.username}
							<span class="flex items-center gap-1">
								<span class="font-mono">{item.username}</span>
								<button
									type="button"
									class="hover:text-fg"
									aria-label="Copy the username of {item.name}"
									onclick={() => copy(item.username, 'Username')}
								>
									<Copy size={12} />
								</button>
							</span>
						{/if}
						{#if item.url}
							{#if isLink(item.url)}
								<!-- eslint-disable svelte/no-navigation-without-resolve -- an outside address -->
								<a
									class="underline hover:text-fg"
									href={item.url}
									target="_blank"
									rel="noopener noreferrer"
								>
									{item.url}
								</a>
								<!-- eslint-enable svelte/no-navigation-without-resolve -->
							{:else}
								<span class="font-mono">{item.url}</span>
							{/if}
						{/if}
					</div>
					{#if shown?.id === item.id}
						<div
							class="flex items-center gap-2 rounded-control bg-surface-sunken px-3 py-2"
							data-testid="vault-secret"
						>
							<code class="min-w-0 flex-1 font-mono text-sm break-all whitespace-pre-wrap text-fg">
								{shown.secret}
							</code>
							<Button size="sm" onclick={() => shown && copy(shown.secret, 'Secret')}>
								<Copy size={14} /> Copy
							</Button>
						</div>
					{/if}
					{#if item.notes}<p class="text-sm whitespace-pre-wrap text-fg">{item.notes}</p>{/if}
					<p class="text-xs text-fg-subtle">
						{item.lastRevealedAt ? `Last revealed ${when(item.lastRevealedAt)}` : 'Never revealed'}
						{#if item.secretChangedAt}
							· secret set {when(item.secretChangedAt)}{/if}
					</p>
				</li>
			{/each}
		</ul>
	{/if}
</Card>

<Dialog bind:open={revealOpen} title="Reveal {revealing?.name ?? ''}">
	<form
		class="flex flex-col gap-4"
		onsubmit={(e) => {
			e.preventDefault();
			void reveal();
		}}
	>
		<p class="text-sm text-fg-muted">The reveal is recorded with your name and the reason.</p>
		<Field label="Reason" for="vault-reason" hint="Optional, e.g. the ticket you're working">
			<Input id="vault-reason" bind:value={reason} maxlength={500} autocomplete="off" />
		</Field>
		<div class="flex justify-end gap-2">
			<Button variant="ghost" onclick={() => (revealOpen = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Reveal</Button>
		</div>
	</form>
</Dialog>

<Dialog bind:open title={editing ? `Edit ${editing.name}` : 'Add a password'}>
	<form
		method="POST"
		action="?tab=vault&/vaultSave"
		use:enhance={done(editing ? 'Saved.' : 'Added.')}
		class="flex flex-col gap-4"
		autocomplete="off"
	>
		{#if editing}
			<input type="hidden" name="itemId" value={editing.id} />
			<input type="hidden" name="version" value={editing.version} />
		{/if}
		<input type="hidden" name="tab" value="vault" />
		<div class="grid gap-4 sm:grid-cols-2">
			<Field label="Name" for="vault-name" error={errors.name}>
				<Input id="vault-name" name="name" required maxlength={200} value={editing?.name ?? ''} />
			</Field>
			<Field label="Kind" for="vault-kind" error={errors.kind}>
				<Select id="vault-kind" name="kind" options={KINDS} value={editing?.kind ?? 'login'} />
			</Field>
			<Field label="Username" for="vault-username" error={errors.username}>
				<Input
					id="vault-username"
					name="username"
					maxlength={500}
					value={editing?.username ?? ''}
					autocomplete="off"
				/>
			</Field>
			<Field label="Address" for="vault-url" hint="A URL, host or IP" error={errors.url}>
				<Input id="vault-url" name="url" maxlength={2000} value={editing?.url ?? ''} />
			</Field>
			<Field
				label="Secret"
				for="vault-secret"
				class="sm:col-span-2"
				hint={editing?.hasSecret
					? 'Leave empty to keep the current secret'
					: 'The password, key or secret note: stored encrypted'}
				error={errors.secret}
			>
				<Input id="vault-secret" name="secret" type="password" autocomplete="new-password" />
			</Field>
			{#if editing?.hasSecret}
				<label class="flex items-center gap-2 text-sm text-fg sm:col-span-2">
					<input type="checkbox" name="clearSecret" /> Remove the secret
				</label>
			{/if}
			<Field
				label="Notes"
				for="vault-notes"
				class="sm:col-span-2"
				hint="Not secret: how to use it, who owns it"
				error={errors.notes}
			>
				<textarea
					id="vault-notes"
					name="notes"
					rows="3"
					maxlength="10000"
					class="w-full rounded-control border border-border-strong bg-surface px-3 py-2 text-sm text-fg"
					>{editing?.notes ?? ''}</textarea
				>
			</Field>
			<Field
				label="Tags"
				for="vault-tags"
				hint="Separated by commas"
				error={errors.tags}
				class="sm:col-span-2"
			>
				<Input id="vault-tags" name="tags" value={editing?.tags.join(', ') ?? ''} />
			</Field>
		</div>
		<div class="flex justify-between gap-2">
			<div>
				{#if editing}
					<Button type="submit" variant="danger" formaction="?tab=vault&/vaultDelete">
						Delete
					</Button>
				{/if}
			</div>
			<div class="flex gap-2">
				<Button variant="ghost" onclick={() => (open = false)}>Cancel</Button>
				<Button type="submit" variant="primary">Save</Button>
			</div>
		</div>
	</form>
</Dialog>
