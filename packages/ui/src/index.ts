// Huxley design system. Import theme tokens separately: `@usehuxley/ui/theme.css`.

export { default as AppShell, type NavItem } from './components/AppShell.svelte';
export { default as Badge, type BadgeTone } from './components/Badge.svelte';
export { default as Button, type ButtonSize, type ButtonVariant } from './components/Button.svelte';
export { default as Card } from './components/Card.svelte';
export { default as CommandPalette, type Command } from './components/CommandPalette.svelte';
export { default as DataTable, type Column } from './components/DataTable.svelte';
export { default as Dialog } from './components/Dialog.svelte';
export { default as Drawer } from './components/Drawer.svelte';
export { default as EmptyState } from './components/EmptyState.svelte';
export { default as Field } from './components/Field.svelte';
export { default as Input } from './components/Input.svelte';
export { default as Select, type SelectOption } from './components/Select.svelte';
export { default as Tabs, type TabItem } from './components/Tabs.svelte';
export { default as ThemeToggle, type ThemePreference } from './components/ThemeToggle.svelte';
export { default as Toaster } from './components/Toaster.svelte';
export { default as Workbench } from './components/Workbench.svelte';
export { default as HuxleyMark } from './components/HuxleyMark.svelte';
export type { Space, SpaceMenuGroup, SpaceMenuItem } from './workbench';
export { tray, clockTone, clockUsed, type TrayItem, type TrayKind } from './tray.svelte';
export { toast, type Toast, type ToastTone } from './toast.svelte';
export { default as FormRenderer } from './form/FormRenderer.svelte';
export {
	answersOf,
	fieldsOf,
	shownFields,
	type Choice,
	type Field as FormField,
	type FieldKind,
	type ShowIf
} from './form/fields';
export { default as ArticleBody } from './components/ArticleBody.svelte';
export { default as ReportBody } from './components/ReportBody.svelte';

export { default as CodeEditor } from './components/CodeEditor.svelte';
export type {
	CodeCompletion,
	CodeDiagnostic,
	CodeLanguage,
	JinjaScope
} from './components/code-editor/types.ts';
