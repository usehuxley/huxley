<script lang="ts">
	import { Field, Input } from '@usehuxley/ui';
	import HoursFields from './HoursFields.svelte';

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
<div class="mt-4">
	<HoursFields hours={site?.hours ?? null} error={errors.hours} />
</div>
