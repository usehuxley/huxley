import type { BadgeTone } from '@usehuxley/ui';

/** Platform status as shown on the home page. */
export type PlatformStatus =
	| { state: 'ready'; version: string }
	| { state: 'degraded'; detail: string }
	| { state: 'unreachable' };

/** Maps a `/readyz` outcome to a display status. */
export function toPlatformStatus(
	result: { ok: true; version: string } | { ok: false; detail?: string } | null
): PlatformStatus {
	if (result === null) return { state: 'unreachable' };
	if (result.ok) return { state: 'ready', version: result.version };
	return { state: 'degraded', detail: result.detail ?? 'A dependency is unavailable.' };
}

export function statusBadge(status: PlatformStatus): { tone: BadgeTone; label: string } {
	switch (status.state) {
		case 'ready':
			return { tone: 'success', label: 'Operational' };
		case 'degraded':
			return { tone: 'warning', label: 'Degraded' };
		case 'unreachable':
			return { tone: 'danger', label: 'API unreachable' };
	}
}
