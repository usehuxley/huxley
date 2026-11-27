<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Eraser } from '@lucide/svelte';
	import { Badge, Button, Card, Field, Input } from '@usehuxley/ui';
	import { BILLING, STATUS, money } from '#lib/quotes/format.ts';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let q = $derived(data.quote);
	let status = $derived(STATUS[q.status] ?? STATUS.sent);
	let open = $derived(q.status === 'sent' && !q.expired);
	let items = $derived(
		q.lines.filter((l) => !l.optional || (q.status === 'accepted' && l.selected))
	);
	let options = $derived(q.lines.filter((l) => l.optional && l.kind === 'item'));
	let taken = $state<string[]>([]);
	let declining = $state(false);

	// The totals with the options ticked so far.
	let totals = $derived.by(() => {
		let oneTime = Number(q.totals.oneTime);
		let monthly = Number(q.totals.monthly);
		for (const o of options) {
			if (!taken.includes(o.id)) continue;
			const t = Number(o.total);
			if (o.billing === 'one_time') oneTime += t;
			else monthly += o.billing === 'monthly' ? t : o.billing === 'quarterly' ? t / 3 : t / 12;
		}
		return { oneTime, monthly };
	});

	// A signature pad: draw with a mouse, pen or finger.
	let canvas: HTMLCanvasElement | undefined = $state();
	let signature = $state('');
	let drawing = false;
	let last: { x: number; y: number } | null = null;

	function point(e: PointerEvent) {
		const r = canvas!.getBoundingClientRect();
		return {
			x: ((e.clientX - r.left) / r.width) * canvas!.width,
			y: ((e.clientY - r.top) / r.height) * canvas!.height
		};
	}
	function down(e: PointerEvent) {
		if (!canvas) return;
		try {
			canvas.setPointerCapture(e.pointerId);
		} catch {
			// Not every pointer can be captured; drawing still works without it.
		}
		drawing = true;
		last = point(e);
	}
	function move(e: PointerEvent) {
		if (!drawing || !canvas || !last) return;
		const ctx = canvas.getContext('2d');
		if (!ctx) return;
		const p = point(e);
		ctx.strokeStyle = '#13171c';
		ctx.lineWidth = 2.5;
		ctx.lineCap = 'round';
		ctx.beginPath();
		ctx.moveTo(last.x, last.y);
		ctx.lineTo(p.x, p.y);
		ctx.stroke();
		last = p;
	}
	function up() {
		if (!drawing || !canvas) return;
		drawing = false;
		last = null;
		signature = canvas.toDataURL('image/png');
	}
	function clear() {
		canvas?.getContext('2d')?.clearRect(0, 0, canvas.width, canvas.height);
		signature = '';
	}
</script>

<svelte:head>
	<title>{q.title} · Quotes · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-4xl flex-col gap-6">
	<div>
		<a href={resolve('/(portal)/quotes')} class="text-sm text-fg-muted hover:text-fg">← Quotes</a>
		<h1 class="mt-2 text-2xl font-semibold text-fg">{q.title}</h1>
		<div class="mt-2 flex flex-wrap items-center gap-2 text-sm text-fg-muted">
			<Badge tone={q.expired ? 'neutral' : status.tone}>
				{q.expired ? 'Expired' : status.label}
			</Badge>
			<span>
				Q-{q.number}{q.validUntil
					? ` · valid until ${new Date(`${q.validUntil}T00:00:00`).toLocaleDateString()}`
					: ''}
			</span>
			{#if q.ownerName}<span>· from {q.ownerName}</span>{/if}
		</div>
	</div>

	{#if q.intro}
		<p class="text-sm whitespace-pre-wrap text-fg">{q.intro}</p>
	{/if}

	<Card>
		<table class="w-full text-left text-sm" aria-label="What's quoted">
			<thead class="border-b border-border text-xs font-bold text-fg-muted">
				<tr>
					<th scope="col" class="py-2 pr-3">Item</th>
					<th scope="col" class="py-2 pr-3 text-right">Quantity</th>
					<th scope="col" class="py-2 pr-3 text-right">Price</th>
					<th scope="col" class="py-2 text-right">Total</th>
				</tr>
			</thead>
			<tbody>
				{#each items as l (l.id)}
					{#if l.kind === 'heading'}
						<tr>
							<th colspan="4" scope="colgroup" class="pt-4 pb-1 text-left font-semibold text-fg">
								{l.name}
							</th>
						</tr>
					{:else}
						<tr class="border-b border-border last:border-0">
							<td class="py-2 pr-3">
								<span class="font-medium text-fg">{l.name}</span>
								{#if l.description}<span class="block text-xs text-fg-muted">
										{l.description}
									</span>{/if}
							</td>
							<td class="py-2 pr-3 text-right tabular-nums">{Number(l.quantity)} {l.unit}</td>
							<td class="py-2 pr-3 text-right tabular-nums">
								{money(l.unitPrice, q.currency)}{#if Number(l.discountPct) > 0}<span
										class="block text-xs text-success"
									>
										{Number(l.discountPct)}% off
									</span>{/if}
							</td>
							<td class="py-2 text-right font-medium tabular-nums">
								{money(l.total, q.currency)}<span class="block text-xs font-normal text-fg-muted">
									{BILLING[l.billing]}
								</span>
							</td>
						</tr>
					{/if}
				{/each}
			</tbody>
		</table>
		<dl
			class="mt-4 ml-auto grid max-w-xs grid-cols-[1fr_auto] gap-x-6 gap-y-1 text-sm"
			aria-label="Totals"
		>
			<dt class="text-fg-muted">One-time</dt>
			<dd class="text-right font-semibold tabular-nums">{money(totals.oneTime, q.currency)}</dd>
			<dt class="text-fg-muted">Monthly</dt>
			<dd class="text-right font-semibold tabular-nums">{money(totals.monthly, q.currency)}</dd>
		</dl>
	</Card>

	{#if q.scope}
		<Card title="Scope of work">
			<div class="flex flex-col gap-4 text-sm" aria-label="Scope of work">
				{#each q.scope.sections as x, i (i)}
					<section>
						<h2 class="font-semibold text-fg">{x.title}</h2>
						<p class="mt-1 whitespace-pre-wrap text-fg">{x.body}</p>
					</section>
				{/each}
			</div>
		</Card>
	{/if}

	{#if q.terms}
		<Card title="Terms"><p class="text-sm whitespace-pre-wrap text-fg">{q.terms}</p></Card>
	{/if}

	{#if form?.answered || q.status !== 'sent'}
		<div role="status" class="rounded-card border border-border bg-surface p-4 text-sm text-fg">
			{#if q.status === 'accepted'}
				Accepted by {q.decidedBy}{q.decidedAt
					? ` on ${new Date(q.decidedAt).toLocaleDateString()}`
					: ''}. Thank you, we'll be in touch about next steps.
			{:else if q.status === 'declined'}
				Declined by {q.decidedBy}. Thanks for letting us know.
			{/if}
		</div>
	{:else if q.expired}
		<div role="status" class="rounded-card border border-border bg-surface p-4 text-sm text-fg">
			This quote has expired. Reply to your contact for an up-to-date one.
		</div>
	{:else if open}
		<Card title={declining ? 'Decline this quote' : 'Accept this quote'}>
			<form method="POST" class="flex flex-col gap-4" use:enhance>
				<input type="hidden" name="revision" value={q.revision} />
				<input type="hidden" name="decision" value={declining ? 'decline' : 'accept'} />
				{#if !declining && options.length > 0}
					<fieldset class="flex flex-col gap-2">
						<legend class="mb-1 text-sm font-semibold text-fg">Options</legend>
						{#each options as o (o.id)}
							<label class="flex items-center gap-2 text-sm text-fg">
								<input type="checkbox" name="options" value={o.id} bind:group={taken} />
								{o.name} · {money(o.total, q.currency)}
								{BILLING[o.billing]}
							</label>
						{/each}
					</fieldset>
				{/if}
				<Field label="Your full name" for="name">
					<Input id="name" name="name" required maxlength={200} autocomplete="name" />
				</Field>
				{#if declining}
					<Field label="Anything we should know?" for="reason">
						<Input id="reason" name="reason" maxlength={2000} />
					</Field>
				{:else}
					<div>
						<p class="mb-1 text-sm font-semibold text-fg" id="sign-label">Sign here</p>
						<canvas
							bind:this={canvas}
							width="600"
							height="160"
							aria-labelledby="sign-label"
							class="block h-40 w-full max-w-xl touch-none rounded-control border border-border-strong bg-white"
							onpointerdown={down}
							onpointermove={move}
							onpointerup={up}
							onpointerleave={up}
						></canvas>
						<button
							type="button"
							class="mt-1 inline-flex items-center gap-1 text-xs text-fg-muted hover:text-fg"
							onclick={clear}
						>
							<Eraser size={12} /> Clear
						</button>
						<input type="hidden" name="signature" value={signature} />
					</div>
					<label class="flex items-start gap-2 text-sm text-fg">
						<input type="checkbox" name="agree" required class="mt-0.5" />
						I accept this quote and its terms on behalf of my organization.
					</label>
				{/if}
				{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
				<div class="flex flex-wrap gap-2">
					{#if declining}
						<Button type="submit" variant="primary">Decline the quote</Button>
						<Button type="button" variant="ghost" onclick={() => (declining = false)}>Back</Button>
					{:else}
						<Button type="submit" variant="primary" disabled={!signature}>Accept and sign</Button>
						<Button type="button" variant="ghost" onclick={() => (declining = true)}>
							Decline instead
						</Button>
					{/if}
				</div>
			</form>
		</Card>
	{/if}
</div>
