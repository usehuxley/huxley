/**
 * The fields a connection asks for, per auth kind. Mirrors
 * `huxley_integrations::connections::fields`.
 */

export interface ConnectionField {
	name: string;
	label: string;
	secret: boolean;
	hint?: string;
}

const FIELDS: Record<string, ConnectionField[]> = {
	apiKey: [{ name: 'key', label: 'API key', secret: true }],
	basic: [
		{ name: 'username', label: 'Username', secret: false },
		{ name: 'password', label: 'Password', secret: true }
	],
	bearer: [{ name: 'token', label: 'Token', secret: true }],
	oauth: [
		{ name: 'client_id', label: 'Client ID', secret: false },
		{ name: 'client_secret', label: 'Client secret', secret: true },
		{ name: 'scopes', label: 'Scopes', secret: false, hint: 'Separated by spaces' }
	]
};

/** The fields for an auth scheme's `type`. */
export function fieldsFor(type: string | undefined): ConnectionField[] {
	if (!type) return [];
	if (type === 'apiKey' || type === 'basic' || type === 'bearer') return FIELDS[type];
	return FIELDS.oauth;
}

export interface ConnectionBody {
	name: string;
	key?: string | null;
	authScheme?: string | null;
	baseUrl?: string | null;
	serverVariables: Record<string, string>;
	settings: Record<string, string | string[]>;
	secrets: Record<string, string>;
	shared: boolean;
}

/**
 * A connection request from a form: `name`, `authScheme`, `baseUrl`,
 * `shared`, `var.<name>` for server variables, and `field.<name>` for
 * the scheme's fields.
 */
export function connectionFromForm(form: FormData, schemeType: string | undefined): ConnectionBody {
	const text = (name: string) => {
		const value = form.get(name);
		return typeof value === 'string' ? value.trim() : '';
	};
	const serverVariables: Record<string, string> = {};
	for (const [name, value] of form.entries()) {
		if (name.startsWith('var.') && typeof value === 'string' && value.trim()) {
			serverVariables[name.slice(4)] = value.trim();
		}
	}
	const settings: Record<string, string | string[]> = {};
	const secrets: Record<string, string> = {};
	for (const field of fieldsFor(schemeType)) {
		const value = text(`field.${field.name}`);
		if (!value) continue;
		if (field.secret) secrets[field.name] = value;
		else if (field.name === 'scopes') settings.scopes = value.split(/\s+/);
		else settings[field.name] = value;
	}
	return {
		name: text('name'),
		key: text('key') || null,
		authScheme: text('authScheme') || null,
		baseUrl: text('baseUrl') || null,
		serverVariables,
		settings,
		secrets,
		shared: form.get('shared') === 'on'
	};
}
