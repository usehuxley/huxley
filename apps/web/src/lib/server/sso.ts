import type { components } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';

type ProviderInput = components['schemas']['ProviderInput'];
type Preset = components['schemas']['Preset'];
type MfaTrust = components['schemas']['MfaTrust'];

const PRESETS: Preset[] = ['entra', 'google', 'okta', 'authentik', 'keycloak', 'generic'];
const MFA_TRUST: MfaTrust[] = ['amr', 'always', 'never'];

/** Reads the provider form (ProviderFields.svelte) into the API's input. */
export function parseProviderForm(form: FormData): ProviderInput {
	const preset = formString(form, 'preset') as Preset;
	const mfaTrust = formString(form, 'mfaTrust') as MfaTrust;
	const secret = formString(form, 'clientSecret');
	return {
		key: formString(form, 'key').trim(),
		name: formString(form, 'name').trim(),
		preset: PRESETS.includes(preset) ? preset : 'generic',
		issuerUrl: formString(form, 'issuerUrl').trim() || null,
		tenantId: formString(form, 'tenantId').trim() || null,
		clientId: formString(form, 'clientId').trim(),
		// Empty means "keep the stored secret" on update.
		clientSecret: secret === '' ? null : secret,
		enabled: form.get('enabled') === 'on',
		jitProvisioning: form.get('jitProvisioning') === 'on',
		allowedDomains: formString(form, 'allowedDomains')
			.split(/[\s,]+/)
			.filter(Boolean),
		trustEmail: form.get('trustEmail') === 'on',
		roleClaim: formString(form, 'roleClaim').trim(),
		mfaTrust: MFA_TRUST.includes(mfaTrust) ? mfaTrust : 'amr'
	};
}

/** Field errors from a 422 problem, keyed by field name. */
export function fieldErrors(problem: unknown): Record<string, string> {
	const errors: Record<string, string> = {};
	const list = (problem as { errors?: { pointer: string; message: string }[] } | undefined)?.errors;
	for (const e of list ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
	return errors;
}
