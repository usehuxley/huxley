<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { visibleSections } from '#lib/permissions.ts';
	import type { LayoutProps } from './$types';

	let { children, data }: LayoutProps = $props();

	let sections = $derived(visibleSections(data.user.grantedPermissions, data.user.activeModules));
	const hrefs = {
		organizations: resolve('/(app)/settings/organizations'),
		users: resolve('/(app)/settings/users'),
		teams: resolve('/(app)/settings/teams'),
		roles: resolve('/(app)/settings/roles'),
		'service-accounts': resolve('/(app)/settings/service-accounts'),
		audit: resolve('/(app)/settings/audit'),
		fields: resolve('/(app)/settings/fields'),
		queues: resolve('/(app)/settings/queues'),
		sla: resolve('/(app)/settings/sla'),
		triage: resolve('/(app)/settings/triage'),
		'project-templates': resolve('/(app)/settings/project-templates'),
		mailboxes: resolve('/(app)/settings/mailboxes'),
		sales: resolve('/(app)/settings/sales'),
		inventory: resolve('/(app)/settings/inventory'),
		skills: resolve('/(app)/settings/skills'),
		calendars: resolve('/(app)/settings/calendars'),
		responses: resolve('/(app)/settings/responses'),
		templates: resolve('/(app)/settings/templates'),
		'work-types': resolve('/(app)/settings/work-types'),
		'asset-types': resolve('/(app)/settings/asset-types'),
		'asset-sync': resolve('/(app)/settings/asset-sync'),
		modules: resolve('/(app)/settings/modules'),
		jobs: resolve('/(app)/settings/jobs'),
		runners: resolve('/(app)/settings/runners'),
		packs: resolve('/(app)/settings/packs'),
		ai: resolve('/(app)/settings/ai'),
		security: resolve('/(app)/settings/security'),
		sso: resolve('/(app)/settings/sso')
	};
</script>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">Settings</h1>
		<p class="mt-1 text-fg-muted">Organizations, people, and access.</p>
	</div>

	<nav aria-label="Settings sections" class="flex gap-1 overflow-x-auto border-b border-border">
		{#each sections as section (section.key)}
			{@const href = hrefs[section.key]}
			{@const active = page.url.pathname.startsWith(href)}
			<a
				{href}
				aria-current={active ? 'page' : undefined}
				class="-mb-px shrink-0 border-b-2 px-3 py-2 text-sm font-medium whitespace-nowrap {active
					? 'border-accent text-fg'
					: 'border-transparent text-fg-muted hover:text-fg'}"
			>
				{section.label}
			</a>
		{/each}
	</nav>

	{@render children()}
</div>
