import { error, fail } from '@sveltejs/kit';
import { api } from '#lib/server/api.ts';
import { qrSvg } from '#lib/server/qr.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const { data } = await api(event).GET('/api/portal/v1/me/mfa');
	if (!data) error(503, 'The portal can’t show this right now.');
	return { mfa: data };
};

const code = (form: FormData) => {
	const v = form.get('code');
	return typeof v === 'string' ? v.trim() : '';
};

const problem = (e: unknown, fallback: string) =>
	(e as { errors?: { message: string }[]; detail?: string } | undefined)?.errors?.[0]?.message ??
	(e as { detail?: string } | undefined)?.detail ??
	fallback;

export const actions: Actions = {
	begin: async (event) => {
		const { data, error: e, response } = await api(event).POST('/api/portal/v1/me/mfa/totp');
		if (!data) return fail(response.status, { message: problem(e, 'Could not start.') });
		return { setup: { secret: data.secret, qr: await qrSvg(data.otpauthUri) } };
	},
	confirm: async (event) => {
		const form = await event.request.formData();
		const {
			data,
			error: e,
			response
		} = await api(event).POST('/api/portal/v1/me/mfa/totp/confirm', { body: { code: code(form) } });
		if (!data) {
			const secret = form.get('secret');
			const qr = form.get('qr');
			return fail(response.status, {
				message: problem(e, 'That code didn’t work.'),
				setup: typeof secret === 'string' && typeof qr === 'string' ? { secret, qr } : undefined
			});
		}
		return { recoveryCodes: data.recoveryCodes };
	},
	regenerate: async (event) => {
		const {
			data,
			error: e,
			response
		} = await api(event).POST('/api/portal/v1/me/mfa/recovery-codes');
		if (!data) return fail(response.status, { message: problem(e, 'Could not replace them.') });
		return { recoveryCodes: data.recoveryCodes };
	},
	disable: async (event) => {
		const form = await event.request.formData();
		const { error: e, response } = await api(event).POST('/api/portal/v1/me/mfa/totp/disable', {
			body: { code: code(form) }
		});
		if (response.status !== 204) {
			return fail(response.status, { message: problem(e, 'Could not turn it off.') });
		}
		return { disabled: true };
	}
};
