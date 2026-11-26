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

	// Images are sent as data URLs, read from the chosen file.
	let images: Record<string, string> = $state({});
	let imageErrors: Record<string, string> = $state({});
	function pickImage(key: string, event: Event) {
		const file = (event.currentTarget as HTMLInputElement).files?.[0];
		if (!file) return;
		if (file.size > 200 * 1024) {
			imageErrors = { ...imageErrors, [key]: 'That image is over 200 KB.' };
			return;
		}
		const reader = new FileReader();
		reader.onload = () => {
			if (typeof reader.result !== 'string') return;
			images = { ...images, [key]: reader.result };
			imageErrors = { ...imageErrors, [key]: '' };
		};
		reader.readAsDataURL(file);
	}

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
								{:else if s.kind === 'image'}
									{@const shown = images[s.key] ?? s.value}
									{#if shown}<img src={shown} alt="" class="h-9 max-w-32 object-contain" />{/if}
									<input type="hidden" name="value" value={images[s.key] ?? s.value ?? ''} />
									{#if editable}
										<input
											{id}
											type="file"
											accept="image/png,image/jpeg,image/webp,image/svg+xml"
											class="max-w-56 text-sm text-fg-muted"
											onchange={(e) => pickImage(s.key, e)}
										/>
									{/if}
									{#if imageErrors[s.key]}<span class="text-sm text-danger">
											{imageErrors[s.key]}
										</span>{/if}
								{:else if s.kind === 'toggle'}
									<Select
										{id}
										name="value"
										value={s.value ?? 'false'}
										options={[
											{ value: 'true', label: 'On' },
											{ value: 'false', label: 'Off' }
										]}
										disabled={!editable}
									/>
								{:else if s.kind === 'percent'}
									<span class="flex items-center gap-1">
										<Input
											{id}
											name="value"
											type="number"
											min="0"
											max="100"
											step="1"
											value={s.value ?? ''}
											disabled={!editable}
											class="w-24"
										/>
										<span class="text-sm text-fg-muted">%</span>
									</span>
								{:else if s.kind === 'hostname'}
									<Input
										{id}
										name="value"
										value={s.value ?? ''}
										placeholder="support.example.com"
										maxlength={253}
										disabled={!editable}
										class="w-56"
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
