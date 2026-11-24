/** Permission keys the UI uses to decide what to show. The API enforces them. */
export const SETTINGS_SECTIONS = [
	{ key: 'organizations', label: 'Organizations', permission: 'org:read' },
	{ key: 'users', label: 'Users', permission: 'user:read' },
	{ key: 'teams', label: 'Teams', permission: 'user:read' },
	{ key: 'roles', label: 'Roles', permission: 'role:read' },
	{ key: 'service-accounts', label: 'Service accounts', permission: 'service_account:manage' },
	{ key: 'audit', label: 'Audit log', permission: 'audit:read' },
	{ key: 'fields', label: 'Fields & tags', permission: 'custom_field:manage' },
	{
		key: 'queues',
		label: 'Service queues',
		permission: 'servicedesk:configure',
		module: 'servicedesk'
	},
	{ key: 'modules', label: 'Modules', permission: 'platform:modules:manage' },
	{ key: 'jobs', label: 'Workflow jobs', permission: 'platform:jobs:manage' },
	{ key: 'runners', label: 'Script runners', permission: 'platform:runners:manage' },
	{ key: 'packs', label: 'Packs', permission: 'pack:install' },
	{ key: 'ai', label: 'AI', permission: 'ai:manage' },
	{ key: 'security', label: 'Security', permission: 'security:policy:manage' },
	{ key: 'sso', label: 'Single sign-on', permission: 'security:policy:manage' }
] as const;

export type SettingsSection = (typeof SETTINGS_SECTIONS)[number];

/** Sections the user may see; a module's sections only while it's on. */
export function visibleSections(
	granted: readonly string[],
	modules: readonly string[] = []
): SettingsSection[] {
	return SETTINGS_SECTIONS.filter(
		(s) => granted.includes(s.permission) && (!('module' in s) || modules.includes(s.module))
	);
}
