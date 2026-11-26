<!-- Skill gaps: thin coverage, unqualified work, expiring certifications. -->
<script lang="ts">
	import { Badge, Card, EmptyState } from '@usehuxley/ui';
	import type { components } from '@usehuxley/api-client';

	let { gaps }: { gaps: components['schemas']['SkillGapInsights'] } = $props();
</script>

<div class="flex flex-col gap-4">
	<Card title="What to act on">
		{#if gaps.summary.length === 0}
			<p class="text-sm text-fg-muted">
				No gaps: every queue and ticket type with requirements has more than one qualified person,
				recent work met its requirements, and no certification expires in the next 60 days.
			</p>
		{:else}
			<ul class="flex list-disc flex-col gap-1 pl-5 text-sm text-fg" aria-label="Findings">
				{#each gaps.summary as line (line)}<li>{line}</li>{/each}
			</ul>
		{/if}
	</Card>

	<Card
		title="Who can do the work"
		description="Queues and ticket types with requirements, and who meets all of them."
	>
		{#if gaps.coverage.length === 0}
			<EmptyState
				title="No requirements yet"
				description="Set what queues and ticket types need under Settings."
			/>
		{:else}
			<div class="overflow-x-auto">
				<table class="w-full text-left text-sm" aria-label="Coverage">
					<thead class="text-fg-muted">
						<tr>
							<th class="py-1 pr-3 font-medium">Work</th>
							<th class="py-1 pr-3 font-medium">Needs</th>
							<th class="py-1 font-medium">Qualified</th>
						</tr>
					</thead>
					<tbody>
						{#each gaps.coverage as c (c.targetId)}
							<tr class="border-t border-border align-top">
								<td class="py-1 pr-3 text-fg">{c.name}</td>
								<td class="py-1 pr-3 text-fg-muted">{c.requirements.join(', ')}</td>
								<td class="py-1 text-fg">
									<span class="flex flex-wrap items-center gap-2">
										<Badge
											tone={c.qualified.length === 0
												? 'danger'
												: c.qualified.length === 1
													? 'warning'
													: 'success'}
										>
											{c.qualified.length} of {c.pool}
										</Badge>
										{c.qualified.join(', ')}
									</span>
								</td>
							</tr>
						{/each}
					</tbody>
				</table>
			</div>
		{/if}
	</Card>

	{#if gaps.unqualified.length > 0}
		<Card
			title="Work done without its requirements"
			description="Of the last 200 tickets resolved in 90 days."
		>
			<ul class="flex flex-col gap-1 text-sm" aria-label="Unqualified work">
				{#each gaps.unqualified as u (u.requirement)}
					<li class="text-fg">
						<strong>{u.requirement}</strong>: {u.tickets} ticket{u.tickets === 1 ? '' : 's'}, by {u.people.join(
							', '
						)}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}

	{#if gaps.expiring.length > 0}
		<Card title="Certifications expiring in 60 days">
			<ul class="flex flex-col gap-1 text-sm" aria-label="Expiring certifications">
				{#each gaps.expiring as e (`${e.userId}:${e.certification}`)}
					<li class="text-fg">
						{e.person}'s {e.certification}, {new Date(
							`${e.expiresOn}T00:00:00`
						).toLocaleDateString()}
						{#if e.uncovers.length > 0}
							<span class="text-danger">
								: nobody else is qualified for {e.uncovers.join(', ')}
							</span>
						{/if}
					</li>
				{/each}
			</ul>
		</Card>
	{/if}
</div>
