import type { Path } from './catalog.ts';

/** What the editor's components share. */
export interface EditorContext {
	/** The whole document (a deep `$state` proxy). */
	document: Record<string, unknown>;
	selected: Path | null;
	readonly: boolean;
	select(path: Path | null): void;
	/** Records that the document changed. */
	changed(): void;
	/** Problems belonging to the step at this path key. */
	problems(key: string): string[];
}
