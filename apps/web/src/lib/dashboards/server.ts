import type { RequestEvent } from '@sveltejs/kit';
import type { components } from '@usehuxley/api-client';
import type { Answer } from '@usehuxley/widgets';
import { api } from '#lib/server/api.ts';
import { effectiveSpec, type Source } from './spec.ts';

type Dashboard = components['schemas']['Dashboard'];

/** The organization a page works in: `?org=` when given, else the top one you can see. */
export async function homeOrg(event: RequestEvent): Promise<string | null> {
	const asked = event.url.searchParams.get('org');
	if (asked) return asked;
	const { data } = await api(event).GET('/api/v1/orgs');
	const orgs = data ?? [];
	return orgs.find((o) => !orgs.some((p) => p.id === o.parentId))?.id ?? null;
}

export interface WidgetResult {
	answer: Answer | null;
	problem: string | null;
}

/** Each widget's numbers, as the viewer may see them. */
export async function answersFor(
	event: RequestEvent,
	dashboard: Pick<Dashboard, 'widgets'>,
	range: string,
	orgId: string | null
): Promise<{ sources: Source[]; results: Record<string, WidgetResult> }> {
	const { data: catalog } = await api(event).GET('/api/v1/analytics/sources');
	const sources = catalog?.sources ?? [];
	const entries = await Promise.all(
		dashboard.widgets.map(async (w): Promise<[string, WidgetResult]> => {
			const source = sources.find((s) => s.key === w.spec.source);
			if (!source) return [w.id, { answer: null, problem: 'Not available to you.' }];
			const { data, error, response } = await api(event).POST('/api/v1/analytics/query', {
				body: effectiveSpec(w, source, range, orgId)
			});
			if (data) return [w.id, { answer: data, problem: null }];
			const detail = (error as { detail?: string; title?: string } | undefined) ?? {};
			return [
				w.id,
				{
					answer: null,
					problem:
						response.status === 403
							? 'Not available to you.'
							: (detail.detail ?? detail.title ?? 'Couldn’t load this.')
				}
			];
		})
	);
	return { sources, results: Object.fromEntries(entries) };
}
