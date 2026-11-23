/**
 * Browser side of passkey ceremonies. The API returns WebAuthn options as
 * JSON (`{ publicKey: … }` with base64url binary fields) and accepts the
 * browser's `PublicKeyCredential.toJSON()` back, so this only bridges the
 * two with the native JSON helpers (Chrome 129+, Firefox 119+, Safari 18.4+).
 */

type Options = { publicKey: Record<string, unknown> };

/** Whether this browser can use passkeys with Huxley. */
export function passkeysSupported(): boolean {
	return (
		typeof window !== 'undefined' &&
		typeof window.PublicKeyCredential !== 'undefined' &&
		typeof PublicKeyCredential.parseRequestOptionsFromJSON === 'function'
	);
}

/** The user dismissed the prompt or it timed out: not worth an error message. */
export function isCancelled(error: unknown): boolean {
	return error instanceof DOMException && error.name === 'NotAllowedError';
}

/** Creates a passkey from registration options. */
export async function createPasskey(options: Options): Promise<unknown> {
	const publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(
		options.publicKey as unknown as PublicKeyCredentialCreationOptionsJSON
	);
	const credential = (await navigator.credentials.create({ publicKey })) as PublicKeyCredential;
	return credential.toJSON();
}

/** Signs a sign-in challenge with a passkey. */
export async function usePasskey(options: Options): Promise<unknown> {
	const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(
		options.publicKey as unknown as PublicKeyCredentialRequestOptionsJSON
	);
	const credential = (await navigator.credentials.get({ publicKey })) as PublicKeyCredential;
	return credential.toJSON();
}

/** POSTs JSON to one of the app's own endpoints and returns the JSON reply. */
export async function postJson<T>(
	url: string,
	body: unknown = {}
): Promise<{ ok: boolean; data: T }> {
	const response = await fetch(url, {
		method: 'POST',
		headers: { 'content-type': 'application/json' },
		body: JSON.stringify(body)
	});
	const data = (await response.json().catch(() => ({}))) as T;
	return { ok: response.ok, data };
}
