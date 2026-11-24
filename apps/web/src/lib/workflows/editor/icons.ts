import {
	Bell,
	Building2,
	Braces,
	CornerDownLeft,
	Database,
	DatabaseZap,
	GitBranch,
	GitFork,
	Globe,
	Hourglass,
	Mail,
	MessageSquareText,
	OctagonX,
	Plug,
	Repeat,
	ScanText,
	ShieldAlert,
	Shuffle,
	Sparkles,
	Split,
	Tags,
	Terminal,
	UserCheck,
	UserRoundSearch,
	UserRoundPen,
	Variable,
	Webhook,
	Workflow
} from '@lucide/svelte';

const ICONS: Record<string, typeof Globe> = {
	'http.request': Globe,
	'integration.action': Plug,
	'core.email': Mail,
	'core.notify': Bell,
	script: Terminal,
	'ai.prompt': MessageSquareText,
	'ai.classify': Tags,
	'ai.extract': ScanText,
	'dataset.query': Database,
	'dataset.upsert': DatabaseZap,
	'core.contact.find': UserRoundSearch,
	'core.contact.upsert': UserRoundPen,
	'core.company.get': Building2,
	'core.company.update': Building2,
	transform: Shuffle,
	set_var: Variable,
	subworkflow: Workflow,
	approval: UserCheck,
	wait: Hourglass,
	wait_for_callback: Webhook,
	if: GitBranch,
	switch: Split,
	foreach: Repeat,
	parallel: GitFork,
	try: ShieldAlert,
	return: CornerDownLeft,
	fail: OctagonX
};

/** The icon for a step type (AI steps share a sparkle when unknown). */
export function stepIcon(group: string | undefined, type: unknown): typeof Globe {
	const known = typeof type === 'string' ? ICONS[type] : undefined;
	if (known) return known;
	return typeof type === 'string' && type.startsWith('ai.') ? Sparkles : Braces;
}
