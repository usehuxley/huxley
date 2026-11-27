import { error, fail } from '@sveltejs/kit';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import type { components } from '@usehuxley/api-client';
import type { Actions, PageServerLoad } from './$types';

type RuleKind = components['schemas']['InvoiceReviewRuleKind'];

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const orgs = await client.GET('/api/v1/orgs');
	const providers = (orgs.data ?? []).filter((o) => o.kind === 'provider');
	const all = await Promise.all(
		providers.map(async (o) => {
			const { data, response } = await client.GET('/api/v1/billing/settings', {
				params: { query: { orgId: o.id } }
			});
			if (response.status === 404) error(404, 'Invoicing is turned off.');
			const sync = await client.GET('/api/v1/billing/sync-settings', {
				params: { query: { orgId: o.id } }
			});
			return { orgId: o.id, name: o.name, settings: data, sync: sync.data ?? null };
		})
	);
	const rules = await client.GET('/api/v1/billing/review-rules');
	const { user } = await event.parent();
	return {
		providers: all,
		rules: rules.data ?? [],
		canRules: user.grantedPermissions.includes('billing:configure')
	};
};

export const actions: Actions = {
	save: async (event) => {
		const form = await event.request.formData();
		const rate = formString(form, 'hourlyRate').trim();
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/billing/settings', {
			body: {
				orgId: formString(form, 'orgId'),
				hourlyRate: rate || null,
				currency: formString(form, 'currency') || 'USD',
				paymentDays: Number(formString(form, 'paymentDays') || '30'),
				approvedTimeOnly: form.get('approvedTimeOnly') === 'on'
			}
		});
		if (!data)
			return fail(response.status, { message: problemMessage(problem, 'Could not save.') });
		return { message: 'Saved.' };
	},
	saveSync: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: problem,
			response
		} = await api(event).PUT('/api/v1/billing/sync-settings', {
			body: {
				orgId: formString(form, 'orgId'),
				target: formString(form, 'target'),
				enabled: form.get('enabled') === 'on',
				onApproval: form.get('onApproval') === 'on',
				creditHoldDays: formString(form, 'creditHoldDays')
					? Number(formString(form, 'creditHoldDays'))
					: null,
				creditHoldAmount: formString(form, 'creditHoldAmount').trim() || null
			}
		});
		if (!data)
			return fail(response.status, { message: problemMessage(problem, 'Could not save.') });
		return { message: 'Saved.' };
	},
	saveRule: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const threshold = formString(form, 'threshold').trim();
		const expression = formString(form, 'expression').trim();
		const body = {
			name: formString(form, 'name'),
			kind: (formString(form, 'kind') || 'edited') as RuleKind,
			threshold: threshold || null,
			expression: expression || null,
			reviewers: formString(form, 'reviewers'),
			active: form.get('active') === 'on'
		};
		const client = api(event);
		const {
			data,
			error: problem,
			response
		} = id
			? await client.PUT('/api/v1/billing/review-rules/{id}', { params: { path: { id } }, body })
			: await client.POST('/api/v1/billing/review-rules', { body });
		if (!data)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not save the rule.')
			});
		return { message: 'Saved.' };
	},
	deleteRule: async (event) => {
		const form = await event.request.formData();
		const { response, error: problem } = await api(event).DELETE(
			'/api/v1/billing/review-rules/{id}',
			{
				params: { path: { id: formString(form, 'id') } }
			}
		);
		if (!response.ok)
			return fail(response.status, {
				message: problemMessage(problem, 'Could not remove the rule.')
			});
		return { message: 'Removed.' };
	}
};
