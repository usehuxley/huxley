<script lang="ts">
	import { enhance } from '$app/forms';
	import { resolve } from '$app/paths';
	import { Button, Card, FormRenderer, answersOf, fieldsOf } from '@usehuxley/ui';
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
	<title>{data.form.name} · {data.me.branding?.name ?? 'Support portal'}</title>
</svelte:head>

<div class="mx-auto flex max-w-2xl flex-col gap-6">
	<div>
		<a href={resolve('/(portal)/requests')} class="text-sm text-accent hover:underline">
			Request something
		</a>
		<h1 class="mt-1 text-2xl font-semibold text-fg">{data.form.name}</h1>
		{#if data.form.description}<p class="mt-1 text-fg-muted">{data.form.description}</p>{/if}
	</div>
	<Card>
		<form method="POST" use:enhance class="flex flex-col gap-4">
			<FormRenderer schema={data.form.schema} ui={data.form.ui} bind:values {errors} />
			<input type="hidden" name="answers" value={answers} />
			<div><Button type="submit" variant="primary">Send request</Button></div>
			{#if form?.message}<p class="text-sm text-danger" role="alert">{form.message}</p>{/if}
		</form>
	</Card>
</div>
