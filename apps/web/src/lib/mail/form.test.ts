import { describe, expect, it } from 'vitest';
import { mailboxBody } from './form';

const form = (fields: Record<string, string>) => {
	const f = new FormData();
	for (const [k, v] of Object.entries(fields)) f.set(k, v);
	return f;
};

describe('mailboxBody', () => {
	it('reads an IMAP mailbox', () => {
		const body = mailboxBody(
			form({
				name: 'Help',
				address: 'help@msp.example',
				kind: 'imap',
				host: 'mail.msp.example',
				port: '993',
				username: 'help',
				password: 'pw',
				stripReplies: 'on',
				enabled: 'on',
				version: '3'
			})
		);
		expect(body.imap).toEqual({
			host: 'mail.msp.example',
			port: 993,
			security: 'tls',
			username: 'help',
			folder: null
		});
		expect(body).toMatchObject({ password: 'pw', stripReplies: true, enabled: true, version: 3 });
	});

	it('leaves out what a forwarded mailbox and a kept password do not need', () => {
		const body = mailboxBody(form({ name: 'F', address: 'f@x.example', kind: 'forward' }));
		expect(body.imap).toBeNull();
		expect(body.password).toBeUndefined();
		expect(body.enabled).toBe(false);
		expect('version' in body).toBe(false);
	});
});
