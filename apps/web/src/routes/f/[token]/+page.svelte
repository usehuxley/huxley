<script lang="ts">
	import { enhance } from '$app/forms';
	import { Button, FormRenderer, answersOf, fieldsOf } from '@usehuxley/ui';
	import type { PageProps } from './$types';

	let { data, form }: PageProps = $props();
	let values = $state<Record<string, unknown>>({});
	let fields = $derived(fieldsOf(data.form.schema, data.form.ui));
	let answers = $derived(JSON.stringify(answersOf(fields, values)));
	let errors: Record<string, string> = $derived(
		form && 'errors' in form && form.errors ? (form.errors as Record<string, string>) : {}
	);
</script>

<svelte:head>
	<title>{data.form.name}</title>
	<meta name="robots" content="noindex" />
</svelte:head>

<main class="flex min-h-screen justify-center bg-bg px-4 py-10">
	<div class="flex w-full max-w-xl flex-col gap-6">
		<header>
			<h1 class="text-2xl font-semibold text-fg">{data.form.name}</h1>
			{#if data.form.description}
				<p class="mt-1 text-sm text-fg-muted">{data.form.description}</p>
			{/if}
		</header>
		{#if form && 'received' in form && form.received}
			<p class="rounded-card border border-border bg-surface p-4 text-sm text-fg" role="status">
				Thanks, your answers were sent.
			</p>
		{:else}
			<form
				method="POST"
				use:enhance
				class="flex flex-col gap-4 rounded-card border border-border bg-surface p-4"
			>
				<FormRenderer schema={data.form.schema} ui={data.form.ui} bind:values {errors} />
				<input type="hidden" name="answers" value={answers} />
				<div><Button type="submit" variant="primary">Send</Button></div>
				{#if form && 'message' in form && form.message}
					<p class="text-sm text-danger" role="alert">{form.message}</p>
				{/if}
			</form>
		{/if}
	</div>
</main>
