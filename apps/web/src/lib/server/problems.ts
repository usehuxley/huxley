import { isApiProblem } from '@usehuxley/api-client';

/** A user-facing message from an API problem response. */
export function problemMessage(error: unknown, fallback: string): string {
	if (isApiProblem(error)) {
		const field = error.errors?.[0]?.message;
		return field ?? error.detail ?? error.title ?? fallback;
	}
	return fallback;
}
