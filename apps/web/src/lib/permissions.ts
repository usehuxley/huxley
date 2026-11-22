/** Permission keys the UI uses to decide what to show. The API enforces them. */
export const SETTINGS_SECTIONS = [
	{ key: 'organizations', label: 'Organizations', permission: 'org:read' },
	{ key: 'users', label: 'Users', permission: 'user:read' },
	{ key: 'audit', label: 'Audit log', permission: 'audit:read' }
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

export function visibleSections(granted: readonly string[]): SettingsSection[] {
	return SETTINGS_SECTIONS.filter((s) => granted.includes(s.permission));
}
