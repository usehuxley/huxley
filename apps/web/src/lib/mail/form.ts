import { formString } from '#lib/forms.ts';

export const KINDS = [
	{ value: 'graph', label: 'Microsoft 365 (Microsoft Graph)' },
	{ value: 'imap', label: 'Fetched over IMAP' },
	{ value: 'forward', label: 'Forwarded to Huxley' }
] as const;

export const SECURITY = [
	{ value: 'tls', label: 'TLS (port 993)' },
	{ value: 'start_tls', label: 'STARTTLS (port 143)' },
	{ value: 'none', label: 'None (your own network only)' }
] as const;

export const UNKNOWN_SENDER = [
	{ value: 'create_contact', label: 'Add them as a contact' },
	{ value: 'no_requester', label: 'Open the ticket without a requester' },
	{ value: 'ignore', label: 'Ignore the message' }
] as const;

export const OUTCOMES: Record<
	string,
	{ label: string; tone: 'success' | 'info' | 'neutral' | 'warning' }
> = {
	ticket_created: { label: 'Ticket opened', tone: 'success' },
	note_added: { label: 'Added to ticket', tone: 'info' },
	ticket_reopened: { label: 'Reopened ticket', tone: 'info' },
	ignored_automatic: { label: 'Automatic mail', tone: 'neutral' },
	ignored_loop: { label: 'Mail loop', tone: 'warning' },
	ignored_unknown_sender: { label: 'Unknown sender', tone: 'neutral' }
};

/** A mailbox from the mailbox form (`MailboxInput`). */
export function mailboxBody(form: FormData) {
	const kind = formString(form, 'kind') || 'forward';
	const port = formString(form, 'port');
	const version = formString(form, 'version');
	return {
		name: formString(form, 'name'),
		address: formString(form, 'address'),
		kind,
		ticketType: formString(form, 'ticketType') || undefined,
		queueId: formString(form, 'queueId') || null,
		unknownSender: formString(form, 'unknownSender') || undefined,
		unknownOrgId: formString(form, 'unknownOrgId') || null,
		stripReplies: form.get('stripReplies') === 'on',
		enabled: form.get('enabled') === 'on',
		imap:
			kind === 'imap'
				? {
						host: formString(form, 'host'),
						port: port ? Number(port) : null,
						security: formString(form, 'security') || 'tls',
						username: formString(form, 'username'),
						folder: formString(form, 'folder') || null
					}
				: null,
		graph:
			kind === 'graph'
				? {
						connection: formString(form, 'graphConnection') || null,
						user: formString(form, 'graphUser') || null,
						folder: formString(form, 'graphFolder') || null
					}
				: null,
		password: formString(form, 'password') || undefined,
		...(version ? { version: Number(version) } : {})
	};
}
