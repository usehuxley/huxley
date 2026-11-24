import { error, fail } from '@sveltejs/kit';
import { isApiProblem } from '@usehuxley/api-client';
import { formString } from '#lib/forms.ts';
import { api } from '#lib/server/api.ts';
import { problemMessage } from '#lib/server/problems.ts';
import { qrSvg } from '#lib/server/qr.ts';
import { parseTokenForm } from '#lib/server/tokens.ts';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async (event) => {
	const client = api(event);
	const [sessions, mfa, passkeys, methods, tokens, permissions, apps] = await Promise.all([
		client.GET('/api/v1/auth/sessions'),
		client.GET('/api/v1/auth/mfa'),
		client.GET('/api/v1/auth/passkeys'),
		client.GET('/api/v1/auth/methods'),
		client.GET('/api/v1/auth/tokens'),
		// Refused while two-factor setup is required; the token card waits.
		client.GET('/api/v1/permissions'),
		client.GET('/api/v1/auth/connected-apps')
	]);
	if (!sessions.data || !mfa.data || !passkeys.data) error(503, 'Could not load your profile.');
	return {
		sessions: sessions.data,
		mfa: mfa.data,
		passkeys: passkeys.data,
		passkeysEnabled: methods.data?.passkeys ?? false,
		emailEnabled: methods.data?.passwordReset ?? false,
		tokens: tokens.data ?? [],
		permissions: permissions.data ?? [],
		connectedApps: apps.data ?? []
	};
};

async function enrollmentView(secret: string, otpauthUri: string) {
	return { secret, otpauthUri, qr: await qrSvg(otpauthUri) };
}

export const actions: Actions = {
	changePassword: async (event) => {
		const form = await event.request.formData();
		const currentPassword = formString(form, 'currentPassword');
		const newPassword = formString(form, 'newPassword');
		const confirmPassword = formString(form, 'confirmPassword');
		if (newPassword !== confirmPassword) {
			return fail(400, {
				password: { errors: { confirmPassword: 'The new passwords do not match.' } }
			});
		}

		const { response, error: problem } = await api(event).POST('/api/v1/auth/password', {
			body: { currentPassword, newPassword }
		});
		if (response.status === 204) {
			return { password: { changed: true } };
		}
		const errors: Record<string, string> = {};
		if (isApiProblem(problem)) {
			for (const e of problem.errors ?? []) errors[e.pointer.replace(/^\//, '')] = e.message;
		}
		if (Object.keys(errors).length === 0)
			errors.form = 'Could not change your password. Try again.';
		return fail(response.status, { password: { errors } });
	},

	startTotp: async (event) => {
		const form = await event.request.formData();
		const password = formString(form, 'password');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/auth/mfa/totp', {
			body: { password }
		});
		if (!data) {
			return fail(response.status, {
				mfa: { error: problemMessage(problem, 'Could not start setup. Try again.') }
			});
		}
		return { mfa: { enrollment: await enrollmentView(data.secret, data.otpauthUri) } };
	},

	confirmTotp: async (event) => {
		const form = await event.request.formData();
		const code = formString(form, 'code');
		// Echoed back from the setup step so a mistyped code can be retried
		// without starting over. It's the user's own secret on their own page.
		const secret = formString(form, 'secret');
		const otpauthUri = formString(form, 'otpauthUri');
		const { data, response } = await api(event).POST('/api/v1/auth/mfa/totp/confirm', {
			body: { code }
		});
		if (!data) {
			return fail(response.status, {
				mfa: {
					enrollment: await enrollmentView(secret, otpauthUri),
					error:
						response.status === 401
							? 'That code is not valid. Check the time on your device and try again.'
							: 'Could not finish setup. Start again.'
				}
			});
		}
		return { mfa: { recoveryCodes: data.recoveryCodes, enabled: true } };
	},

	disableTotp: async (event) => {
		const form = await event.request.formData();
		const password = formString(form, 'password');
		const { error: problem, response } = await api(event).POST('/api/v1/auth/mfa/totp/disable', {
			body: { password }
		});
		if (response.status !== 204) {
			return fail(response.status, {
				mfa: { error: problemMessage(problem, 'Could not turn off two-factor authentication.') }
			});
		}
		return { mfa: { disabled: true } };
	},

	regenerateCodes: async (event) => {
		const form = await event.request.formData();
		const password = formString(form, 'password');
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/auth/mfa/recovery-codes', { body: { password } });
		if (!data) {
			return fail(response.status, {
				mfa: { error: problemMessage(problem, 'Could not create new recovery codes.') }
			});
		}
		return { mfa: { recoveryCodes: data.recoveryCodes } };
	},

	deletePasskey: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).DELETE('/api/v1/auth/passkeys/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (response.status !== 204) {
			return fail(response.status, { passkeys: { error: 'Could not remove the passkey.' } });
		}
		return { passkeys: { removed: true } };
	},

	createToken: async (event) => {
		const form = await event.request.formData();
		const body = parseTokenForm(form);
		const {
			data,
			error: problem,
			response
		} = await api(event).POST('/api/v1/auth/tokens', {
			body
		});
		if (!data) {
			return fail(response.status, {
				tokens: { error: problemMessage(problem, 'Could not create the token.') }
			});
		}
		return { tokens: { created: { name: data.name, secret: data.tokenSecret } } };
	},

	revokeToken: async (event) => {
		const form = await event.request.formData();
		const { response } = await api(event).DELETE('/api/v1/auth/tokens/{id}', {
			params: { path: { id: formString(form, 'id') } }
		});
		if (response.status !== 204) {
			return fail(response.status, { tokens: { error: 'Could not revoke the token.' } });
		}
		return { tokens: { revoked: true } };
	},

	sendVerification: async (event) => {
		const { response } = await api(event).POST('/api/v1/auth/email/verification');
		if (response.status !== 202) {
			return fail(response.status, { verification: { error: 'Could not send the email.' } });
		}
		return { verification: { sent: true } };
	},

	revokeSession: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const { response } = await api(event).DELETE('/api/v1/auth/sessions/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return fail(response.status, { revoke: { failed: true } });
		return { revoke: { done: true } };
	},

	disconnectApp: async (event) => {
		const form = await event.request.formData();
		const id = formString(form, 'id');
		const { response } = await api(event).DELETE('/api/v1/auth/connected-apps/{id}', {
			params: { path: { id } }
		});
		if (response.status !== 204) return fail(response.status, { disconnect: { failed: true } });
		return { disconnect: { done: true } };
	}
};
