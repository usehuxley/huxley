<!-- Submit a personal dashboard for publishing: who should see it. -->
<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, Dialog, Field } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let {
		open = $bindable(false),
		audiences,
		problem = null
	}: {
		open?: boolean;
		audiences: components['schemas']['Audiences'];
		problem?: string | null;
	} = $props();
</script>

<Dialog bind:open title="Submit for publishing">
	<form
		method="POST"
		action="?/submit"
		class="flex flex-col gap-4"
		use:enhance={() =>
			async ({ result, update }) => {
				await update();
				if (result.type === 'success') open = false;
			}}
	>
		<p class="text-sm text-fg-muted">
			Someone who manages dashboards reviews it. Once approved, it's shared as it is now. Leave
			teams and roles empty to share it with everyone who sees dashboards.
		</p>
		{#if audiences.teams.length > 0}
			<fieldset class="flex flex-col gap-1">
				<legend class="mb-1 text-sm font-medium text-fg">Teams</legend>
				{#each audiences.teams as t (t.id)}
					<label class="flex items-center gap-2 text-sm text-fg">
						<input type="checkbox" name="teams" value={t.id} />
						{t.name}
					</label>
				{/each}
			</fieldset>
		{/if}
		<fieldset class="flex flex-col gap-1">
			<legend class="mb-1 text-sm font-medium text-fg">Roles</legend>
			{#each audiences.roles as r (r.id)}
				<label class="flex items-center gap-2 text-sm text-fg">
					<input type="checkbox" name="roles" value={r.id} />
					{r.name}
				</label>
			{/each}
		</fieldset>
		<label class="flex items-center gap-2 text-sm text-fg">
			<input type="checkbox" name="portal" />
			Show it in the client portal too (client admins see their own company's numbers)
		</label>
		<Field label="Note for the reviewer" for="publish-note">
			<textarea
				id="publish-note"
				name="note"
				rows="3"
				maxlength="1000"
				class="w-full rounded-control border border-border bg-surface px-3 py-2 text-sm text-fg"
			></textarea>
		</Field>
		{#if problem}<p class="text-sm text-danger" role="alert">{problem}</p>{/if}
		<div class="flex justify-end gap-2">
			<Button type="button" variant="ghost" onclick={() => (open = false)}>Cancel</Button>
			<Button type="submit" variant="primary">Submit</Button>
		</div>
	</form>
</Dialog>
