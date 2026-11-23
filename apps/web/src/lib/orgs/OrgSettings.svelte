<script lang="ts">
	import { enhance, type SubmitFunction } from '$app/forms';
	import type { components } from '@usehuxley/api-client';
	import { Badge, Button, Input, Select } from '@usehuxley/ui';

	type Setting = components['schemas']['ResolvedSetting'];

	let {
		settings,
		orgId,
		canManage,
		notify
	}: { settings: Setting[]; orgId: string; canManage: boolean; notify: SubmitFunction } = $props();

	let groups = $derived(
		[...new Set(settings.map((s) => s.group))].map((group) => ({
			group,
			items: settings.filter((s) => s.group === group)
		}))
	);
	const timezones = ['UTC', ...Intl.supportedValuesOf('timeZone').filter((z) => z !== 'UTC')].map(
		(z) => ({ value: z, label: z })
	);

	function origin(s: Setting): string {
		if (s.lockedBy) return `Locked by ${s.lockedBy.name}`;
		if (s.source?.id === orgId) return s.local?.locked ? 'Set here and locked below' : 'Set here';
		if (s.source) return `Inherited from ${s.source.name}`;
		return s.value ? 'Default' : 'Not set';
	}
</script>

<div class="flex flex-col gap-6">
	{#each groups as { group, items } (group)}
		<section>
			<h3 class="mb-2 text-sm font-semibold text-fg">{group}</h3>
			<ul class="flex flex-col divide-y divide-border">
				{#each items as s (s.key)}
					{@const editable = canManage && !s.lockedBy}
					{@const id = `setting-${s.key.replaceAll('.', '-')}`}
					<li class="py-3 first:pt-0 last:pb-0" data-testid={id}>
						<form
							method="POST"
							action="?/setting"
							class="flex flex-col gap-2 sm:flex-row sm:items-end sm:justify-between"
							use:enhance={notify}
						>
							<input type="hidden" name="key" value={s.key} />
							<div class="min-w-0 sm:w-1/2">
								<label for={id} class="text-sm font-medium text-fg">{s.label}</label>
								<p class="text-sm text-fg-muted">{s.description}</p>
								<p class="mt-1 flex items-center gap-1.5 text-xs text-fg-muted">
									{#if s.lockedBy}<Badge tone="warning">Locked</Badge>{/if}
									{origin(s)}
								</p>
							</div>
							<div class="flex flex-wrap items-center gap-2">
								{#if s.kind === 'timezone'}
									<Select
										{id}
										name="value"
										value={s.value ?? 'UTC'}
										options={timezones}
										disabled={!editable}
									/>
								{:else if s.kind === 'color'}
									<input
										{id}
										name="value"
										type="color"
										value={s.value ?? '#4f46e5'}
										disabled={!editable}
										class="h-9 w-14 rounded-control border border-border bg-surface"
									/>
								{:else}
									<Input
										{id}
										name="value"
										type={s.kind === 'email' ? 'email' : 'text'}
										value={s.value ?? ''}
										placeholder={s.kind === 'text' ? 'Organization name' : ''}
										maxlength={s.kind === 'prefix' ? 8 : 100}
										disabled={!editable}
										class={s.kind === 'prefix' ? 'w-24 uppercase' : 'w-56'}
									/>
								{/if}
								{#if editable}
									<label class="flex items-center gap-1.5 text-sm text-fg-muted">
										<input type="checkbox" name="locked" checked={s.local?.locked ?? false} />
										Lock
									</label>
									<Button type="submit" size="sm" aria-label="Save {s.label}">Save</Button>
									{#if s.local}
										<Button
											type="submit"
											size="sm"
											formaction="?/clearSetting"
											aria-label="Inherit {s.label}"
										>
											Inherit
										</Button>
									{/if}
								{/if}
							</div>
						</form>
					</li>
				{/each}
			</ul>
		</section>
	{/each}
</div>
