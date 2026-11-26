import { error, fail } from '@sveltejs/kit';
import type { components } from '@usehuxley/api-client';
import { api } from '#lib/server/api.ts';
import type { Actions, PageServerLoad } from './$types';

type Role = components['schemas']['PortalRole'];
type Status = components['schemas']['PortalAccountStatus'];
const ROLES: Role[] = ['client_user', 'client_approver', 'client_admin'];

const text = (form: FormData, name: string) => {
	const v = form.get(name);
	return typeof v === 'string' ? v.trim() : '';
};
const role = (form: FormData): Role | undefined => {
	const r = text(form, 'role');
	return ROLES.includes(r as Role) ? (r as Role) : undefined;
};
const problem = (e: unknown, fallback: string) =>
	(e as { errors?: { message: string }[]; detail?: string } | undefined)?.errors?.[0]?.message ??
	(e as { detail?: string } | undefined)?.detail ??
	fallback;
const sent = (invited: boolean, who: string) =>
	invited ? `${who} will get an email to sign in.` : `${who} has access. No email could be sent.`;

export const load: PageServerLoad = async (event) => {
	const { data, response } = await api(event).GET('/api/portal/v1/people');
	if (!data)
		error(response.status === 403 ? 404 : 503, 'This page is for your organization’s admins.');
	return { people: data };
};

export const actions: Actions = {
	add: async (event) => {
		const form = await event.request.formData();
		const firstName = text(form, 'firstName');
		const lastName = text(form, 'lastName');
		const email = text(form, 'email');
		const access = form.get('access') === 'on' ? role(form) : undefined;
		const {
			data,
			error: e,
			response
		} = await api(event).POST('/api/portal/v1/people', {
			body: { firstName, lastName, email, role: access }
		});
		if (!data) {
			return fail(response.status, {
				message: problem(e, 'They couldn’t be added.'),
				firstName,
				lastName,
				email
			});
		}
		return {
			done: access ? sent(data.invitationSent, firstName) : `${firstName} was added.`
		};
	},
	give: async (event) => {
		const form = await event.request.formData();
		const contact = text(form, 'contact');
		const r = role(form);
		if (!r) return fail(400, { message: 'Choose what they can do.' });
		const {
			data,
			error: e,
			response
		} = await api(event).POST('/api/portal/v1/people/{contact}/access', {
			params: { path: { contact } },
			body: { role: r }
		});
		if (!data) return fail(response.status, { message: problem(e, 'That didn’t work.') });
		return { done: sent(data.invitationSent, text(form, 'name') || 'They') };
	},
	change: async (event) => {
		const form = await event.request.formData();
		const contact = text(form, 'contact');
		const r = role(form);
		const status = text(form, 'status') as Status;
		const version = Number(text(form, 'version')) || undefined;
		if (!r) return fail(400, { message: 'Choose what they can do.' });
		const {
			data,
			error: e,
			response
		} = await api(event).PATCH('/api/portal/v1/people/{contact}/access', {
			params: { path: { contact } },
			body: { role: r, status, version }
		});
		if (!data) return fail(response.status, { message: problem(e, 'That didn’t work.') });
		return {
			done:
				status === 'disabled'
					? `${data.name} can no longer sign in.`
					: `${data.name}’s access is saved.`
		};
	}
};
