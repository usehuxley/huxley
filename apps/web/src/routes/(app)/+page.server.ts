import { api } from '#lib/server/api.ts';
import { toPlatformStatus } from '#lib/status.ts';
import { isApiProblem } from '@usehuxley/api-client';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ fetch }) => {
	try {
		const { data, error } = await api(fetch).GET('/readyz', {
			signal: AbortSignal.timeout(3000)
		});
		if (data) return { status: toPlatformStatus({ ok: true, version: data.version }) };
		return {
			status: toPlatformStatus({
				ok: false,
				detail: isApiProblem(error) ? (error.detail ?? undefined) : undefined
			})
		};
	} catch {
		return { status: toPlatformStatus(null) };
	}
};
