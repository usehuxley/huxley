import type { components } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';

type CreateTokenRequest = components['schemas']['CreateTokenRequest'];

/** Reads TokensCard's create form. `never` expiry is offered for service accounts only. */
export function parseTokenForm(form: FormData): CreateTokenRequest {
	const expiry = formString(form, 'expiresInDays');
	const scopes =
		formString(form, 'scopeMode') === 'all'
			? ['*']
			: form.getAll('scopes').filter((s): s is string => typeof s === 'string');
	return {
		name: formString(form, 'name').trim(),
		scopes,
		expiresInDays: expiry === 'never' ? null : Number(expiry) || 30
	};
}
