<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import { resolve } from '$app/paths';
	import { ArrowLeft } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input, toast } from '@usehuxley/ui';
	import TokensCard from '#lib/tokens/TokensCard.svelte';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let tokenForm = $derived(
		(form && 'tokens' in form ? form.tokens : {}) as {
			created?: { name: string; secret: string };
			error?: string;
		}
	);

	const notify: SubmitFunction = () => {
		return async ({ result, update }) => {
			await update({ reset: false });
			if ((result.type === 'success' || result.type === 'failure') && result.data?.message) {
				toast.push({
					title: String(result.data.message),
					tone: result.type === 'success' ? 'success' : 'danger'
				});
			}
		};
	};
</script>

<svelte:head>
	<title>{data.account.displayName} · Service accounts · Huxley</title>
</svelte:head>

<div class="flex flex-col gap-6">
	<a
		href={resolve('/(app)/settings/service-accounts')}
		class="flex w-fit items-center gap-1 text-sm text-fg-muted hover:text-fg"
	>
		<ArrowLeft size={16} /> All service accounts
	</a>

	<Card
		title={data.account.displayName}
		description="A service account: API tokens only, no sign-in."
	>
		{#snippet actions()}
			<Badge tone={data.account.status === 'active' ? 'success' : 'neutral'}>
				{data.account.status}
			</Badge>
		{/snippet}
		<div class="flex flex-col gap-4">
			<form
				method="POST"
				action="?/update"
				class="flex flex-wrap items-end gap-2"
				use:enhance={notify}
			>
				<Field label="Name" for="name">
					<Input id="name" name="name" value={data.account.displayName} required />
				</Field>
				<Field label="Description" for="description">
					<Input id="description" name="description" value={data.account.description} />
				</Field>
				<Button type="submit">Save</Button>
			</form>
			<div class="flex flex-wrap items-center gap-2">
				<form method="POST" action="?/status" use:enhance={notify}>
					<input
						type="hidden"
						name="status"
						value={data.account.status === 'active' ? 'disabled' : 'active'}
					/>
					<Button
						type="submit"
						size="sm"
						variant={data.account.status === 'active' ? 'danger' : 'secondary'}
					>
						{data.account.status === 'active' ? 'Disable' : 'Enable'}
					</Button>
				</form>
				<a
					href={resolve('/(app)/settings/users/[id]', { id: data.account.id })}
					class="text-sm text-accent hover:underline"
				>
					Manage roles
				</a>
			</div>
		</div>
	</Card>

	<TokensCard
		tokens={data.tokens}
		permissions={data.permissions}
		allowNever
		created={tokenForm.created ?? null}
		error={tokenForm.error ?? null}
		description="Tokens act as this service account, limited to what you choose. They may be set never to expire."
	/>
</div>
