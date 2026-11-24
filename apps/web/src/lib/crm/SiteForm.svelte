<script lang="ts">
	import { Field, Input } from '@usehuxley/ui';
	import { DAYS } from './labels.ts';

	/** A site to edit, or none for a new one. */
	type Site = {
		id: string;
		name: string;
		addressLine1?: string | null;
		addressLine2?: string | null;
		city?: string | null;
		region?: string | null;
		postalCode?: string | null;
		country?: string | null;
		timezone: string;
		phone?: string | null;
		isPrimary: boolean;
		hours: Record<string, string[][]>;
		version: number;
	};

	let {
		site = null,
		errors = {},
		prefix
	}: { site?: Site | null; errors?: Record<string, string>; prefix: string } = $props();

	const zones = (() => {
		try {
			return Intl.supportedValuesOf('timeZone');
		} catch {
			return [];
		}
	})();
	const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
	// A day's first range (more can be set through the API).
	const range = (day: string) => site?.hours?.[day]?.[0] ?? null;
	const id = (name: string) => `${prefix}-${name}`;
</script>

{#if site}
	<input type="hidden" name="id" value={site.id} />
	<input type="hidden" name="version" value={site.version} />
{/if}
<div class="grid gap-3 sm:grid-cols-2">
	<div class="sm:col-span-2">
		<Field label="Name" for={id('name')} required error={errors.name}>
			<Input id={id('name')} name="name" value={site?.name ?? ''} required maxlength={200} />
		</Field>
	</div>
	<div class="sm:col-span-2">
		<Field label="Address" for={id('line1')} error={errors.addressLine1}>
			<Input id={id('line1')} name="addressLine1" value={site?.addressLine1 ?? ''} />
		</Field>
	</div>
	<div class="sm:col-span-2">
		<label for={id('line2')} class="sr-only">Address, second line</label>
		<Input id={id('line2')} name="addressLine2" value={site?.addressLine2 ?? ''} />
	</div>
	<Field label="City" for={id('city')} error={errors.city}>
		<Input id={id('city')} name="city" value={site?.city ?? ''} />
	</Field>
	<Field label="Region" for={id('region')} error={errors.region}>
		<Input id={id('region')} name="region" value={site?.region ?? ''} />
	</Field>
	<Field label="Postal code" for={id('postal')} error={errors.postalCode}>
		<Input id={id('postal')} name="postalCode" value={site?.postalCode ?? ''} />
	</Field>
	<Field label="Country" for={id('country')} hint="Two letters, e.g. GB" error={errors.country}>
		<Input
			id={id('country')}
			name="country"
			value={site?.country ?? ''}
			maxlength={2}
			autocomplete="country"
		/>
	</Field>
	<Field label="Timezone" for={id('tz')} error={errors.timezone}>
		<Input id={id('tz')} name="timezone" value={site?.timezone ?? browserZone} list={id('zones')} />
		<datalist id={id('zones')}>
			{#each zones as zone (zone)}<option value={zone}></option>{/each}
		</datalist>
	</Field>
	<Field label="Phone" for={id('phone')} error={errors.phone}>
		<Input id={id('phone')} name="phone" value={site?.phone ?? ''} type="tel" />
	</Field>
	<label class="flex items-center gap-2 text-sm text-fg sm:col-span-2">
		<input type="checkbox" name="isPrimary" checked={site?.isPrimary ?? false} />
		The company's primary site
	</label>
</div>
<fieldset class="mt-4">
	<legend class="text-sm font-medium text-fg">Opening hours</legend>
	{#if errors.hours}<p class="text-sm text-danger">{errors.hours}</p>{/if}
	<div class="mt-2 flex flex-col gap-1.5">
		{#each DAYS as day (day.key)}
			{@const open = range(day.key)}
			<div class="grid grid-cols-[7rem_1fr_1fr] items-center gap-2 text-sm">
				<label class="flex items-center gap-2 text-fg">
					<input
						type="checkbox"
						name="open_{day.key}"
						checked={site ? open !== null : !['sat', 'sun'].includes(day.key)}
					/>
					{day.label}
				</label>
				<Input
					type="time"
					name="from_{day.key}"
					value={open?.[0] ?? '09:00'}
					aria-label="{day.label} opens"
				/>
				<Input
					type="time"
					name="to_{day.key}"
					value={open?.[1] ?? '17:00'}
					aria-label="{day.label} closes"
				/>
			</div>
		{/each}
	</div>
</fieldset>
