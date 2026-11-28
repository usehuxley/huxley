/**
 * A download the malware scan withholds (ADR-094): the API answers 403
 * (quarantined) or 409 (not scanned clean yet) with a problem document.
 * This turns it into a plain page saying why, instead of "Not found".
 */
export async function withheld(upstream: Response | null): Promise<Response | null> {
	if (!upstream || (upstream.status !== 403 && upstream.status !== 409)) return null;
	const problem = (await upstream.json().catch(() => null)) as {
		type?: string;
		detail?: string;
	} | null;
	const type = problem?.type ?? '';
	if (!/file-(quarantined|scanning|not-scanned)$/.test(type)) return null;
	return new Response(problem?.detail ?? 'This file is withheld by the malware scan.', {
		status: upstream.status,
		headers: {
			'content-type': 'text/plain; charset=utf-8',
			'x-content-type-options': 'nosniff',
			'cache-control': 'no-store'
		}
	});
}
