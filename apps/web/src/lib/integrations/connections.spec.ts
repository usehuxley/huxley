import { describe, expect, it } from 'vitest';
import { connectionFromForm, fieldsFor } from './connections.ts';

describe('connections', () => {
	it('asks for the fields each auth kind needs', () => {
		expect(fieldsFor({ type: 'basic' }).map((f) => f.name)).toEqual(['username', 'password']);
		expect(fieldsFor({ type: 'oAuth2ClientCredentials' }).map((f) => f.name)).toContain(
			'client_secret'
		);
		expect(fieldsFor(undefined)).toEqual([]);
	});

	it('asks a token exchange for the fields it fills, secrets and optional ones marked', () => {
		const scheme = {
			type: 'tokenExchange',
			body: { username: 'username', password: 'password', TwoFactorPasscode: 'two_factor_code' },
			headers: { clientID: 'client_id' },
			secrets: ['password'],
			optional: ['two_factor_code']
		};
		expect(fieldsFor(scheme)).toEqual([
			{ name: 'username', label: 'Username', secret: false, hint: undefined },
			{ name: 'password', label: 'Password', secret: true, hint: undefined },
			{ name: 'two_factor_code', label: 'Two factor code', secret: false, hint: 'Optional' },
			{ name: 'client_id', label: 'Client ID', secret: false, hint: undefined }
		]);
	});

	it('builds a request from a form, splitting settings from secrets', () => {
		const form = new FormData();
		form.set('name', ' Billing ');
		form.set('authScheme', 'oauth');
		form.set('var.tenant', 'contoso');
		form.set('var.region', '');
		form.set('field.client_id', 'app');
		form.set('field.client_secret', 's3cret');
		form.set('field.scopes', 'read  write');
		form.set('shared', 'on');
		expect(connectionFromForm(form, { type: 'oAuth2ClientCredentials' })).toEqual({
			name: 'Billing',
			key: null,
			authScheme: 'oauth',
			baseUrl: null,
			serverVariables: { tenant: 'contoso' },
			settings: { client_id: 'app', scopes: ['read', 'write'] },
			secrets: { client_secret: 's3cret' },
			shared: true
		});
	});
});
