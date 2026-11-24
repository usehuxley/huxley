<script lang="ts">
	import { resolve } from '$app/paths';
	import { BadgeCheck } from '@lucide/svelte';
	import { Badge, EmptyState } from '@usehuxley/ui';
	import { byCategory, levelLabel } from '#lib/workforce/labels.ts';
	import type { PageProps } from './$types';

	let { data }: PageProps = $props();

	// Level and verification by person, then skill.
	let cells = $derived(
		Object.fromEntries(data.matrix.map((c) => [`${c.userId}:${c.skillId}`, c])) as Record<
			string,
			{ level: number; verified: boolean } | undefined
		>
	);
	let groups = $derived(byCategory(data.skills));
	const tabs = [
		{ value: 'people', label: 'People', href: resolve('/(app)/people') },
		{ value: 'matrix', label: 'Skills matrix', href: `${resolve('/(app)/people')}?view=matrix` }
	];
	const shade = ['', 'bg-accent-soft/40', 'bg-accent-soft', 'bg-accent/40', 'bg-accent/70'];
</script>

<svelte:head>
	<title>People · Huxley</title>
</svelte:head>

<div class="mx-auto flex max-w-6xl flex-col gap-6">
	<div>
		<h1 class="text-2xl font-semibold text-fg">People</h1>
		<p class="mt-1 text-fg-muted">Your team's skills and certifications.</p>
	</div>

	<nav aria-label="People views" class="flex gap-1 border-b border-border">
		{#each tabs as tab (tab.value)}
			<!-- eslint-disable svelte/no-navigation-without-resolve -- resolve() plus a query string -->
			<a
				href={tab.href}
				aria-current={data.view === tab.value ? 'page' : undefined}
				class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {data.view === tab.value
					? 'border-accent text-fg'
					: 'border-transparent text-fg-muted hover:text-fg'}"
			>
				{tab.label}
			</a>
			<!-- eslint-enable svelte/no-navigation-without-resolve -->
		{/each}
	</nav>

	{#if data.view === 'people'}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="w-full text-left text-sm" aria-label="People">
				<thead
					class="border-b border-border bg-surface-sunken text-xs tracking-wide text-fg-muted uppercase"
				>
					<tr>
						<th scope="col" class="px-4 py-2.5 font-medium">Name</th>
						<th scope="col" class="px-4 py-2.5 text-right font-medium">Skills</th>
						<th scope="col" class="px-4 py-2.5 text-right font-medium">Certifications</th>
						<th scope="col" class="px-4 py-2.5 font-medium"></th>
					</tr>
				</thead>
				<tbody>
					{#each data.people as person (person.userId)}
						<tr class="border-b border-border last:border-0 hover:bg-surface-sunken">
							<td class="px-4 py-2.5">
								<a
									href={resolve('/(app)/people/[id]', { id: person.userId })}
									class="font-medium text-fg hover:text-accent"
								>
									{person.name}
								</a>
								<span class="block text-xs text-fg-muted">{person.email}</span>
							</td>
							<td class="px-4 py-2.5 text-right tabular-nums">{person.skills}</td>
							<td class="px-4 py-2.5 text-right tabular-nums">{person.certifications}</td>
							<td class="px-4 py-2.5">
								{#if person.expiring > 0}
									<Badge tone="warning">{person.expiring} expiring soon</Badge>
								{/if}
							</td>
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
	{:else if data.skills.length === 0}
		<EmptyState
			title="No skills yet"
			description="Add skills under Settings → Skills & certifications, then people can rate themselves."
		/>
	{:else}
		<section class="overflow-x-auto rounded-card border border-border bg-surface">
			<table class="text-sm" aria-label="Skills matrix">
				<thead class="border-b border-border bg-surface-sunken text-xs text-fg-muted">
					<tr>
						<th scope="col" class="sticky left-0 bg-surface-sunken px-4 py-2 text-left font-medium">
							Person
						</th>
						{#each groups as [category, skills] (category)}
							{#each skills as skill, i (skill.id)}
								<th
									scope="col"
									class="px-2 py-2 text-left font-medium whitespace-nowrap {i === 0
										? 'border-l border-border'
										: ''}"
									title={category}
								>
									{skill.name}
								</th>
							{/each}
						{/each}
					</tr>
				</thead>
				<tbody>
					{#each data.people as person (person.userId)}
						<tr class="border-b border-border last:border-0">
							<th
								scope="row"
								class="sticky left-0 bg-surface px-4 py-2 text-left font-medium whitespace-nowrap"
							>
								<a
									href={resolve('/(app)/people/[id]', { id: person.userId })}
									class="text-fg hover:text-accent"
								>
									{person.name}
								</a>
							</th>
							{#each groups as [, skills] (skills[0]?.id)}
								{#each skills as skill, i (skill.id)}
									{@const cell = cells[`${person.userId}:${skill.id}`]}
									<td
										class="px-2 py-2 text-center tabular-nums {i === 0
											? 'border-l border-border'
											: ''} {cell ? shade[cell.level] : ''}"
										title={cell
											? `${levelLabel(cell.level)}${cell.verified ? ', verified' : ''}`
											: 'None'}
									>
										{#if cell}
											<span class="inline-flex items-center gap-0.5">
												{cell.level}
												{#if cell.verified}<BadgeCheck size={12} aria-label="verified" />{/if}
											</span>
										{/if}
									</td>
								{/each}
							{/each}
						</tr>
					{/each}
				</tbody>
			</table>
		</section>
		<p class="text-xs text-fg-muted">
			Levels: 1 learning, 2 working, 3 proficient, 4 expert. A tick means a lead verified it.
		</p>
	{/if}
</div>
