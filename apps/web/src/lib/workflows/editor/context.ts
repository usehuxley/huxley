import type { Path } from './catalog.ts';

/** An action the workflow's organization can call (the API's ActionOption). */
export interface ActionChoice {
	action: string;
	integration: string;
	integrationName: string;
	name: string;
	method: string;
	path: string;
	inputs: string[];
	required: string[];
	deprecated: boolean;
}

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
	/** Integration actions the workflow can call. */
	actions: ActionChoice[];
}
