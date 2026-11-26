/** Languages the code editor knows; each loads the first time it's used. */
export type CodeLanguage =
	| 'powershell'
	| 'python'
	| 'javascript'
	| 'typescript'
	| 'yaml'
	| 'json'
	| 'markdown'
	| 'jinja'
	| 'text';

/** Something to suggest while typing. */
export type CodeCompletion = {
	label: string;
	/** Shown beside it, e.g. a type. */
	detail?: string;
	/** Shown in the documentation popup. */
	info?: string;
	type?: 'variable' | 'property' | 'function' | 'keyword' | 'constant' | 'type' | 'text';
};

/** For Jinja: names in scope, and what's under a dotted path
 * (`steps.fetch.output.` → `["steps", "fetch", "output"]`). */
export type JinjaScope = {
	variables?: CodeCompletion[];
	properties?: (path: readonly string[]) => CodeCompletion[];
};

/** A problem to underline: offsets into the text. */
export type CodeDiagnostic = {
	from: number;
	to: number;
	severity: 'error' | 'warning' | 'info';
	message: string;
};

export type CodeEditorOptions = {
	value: string;
	language: CodeLanguage;
	readonly: boolean;
	label: string;
	placeholder?: string;
	/** Extra words to complete, read each time (so they can change), e.g.
	 * script parameters. */
	words?: () => CodeCompletion[];
	jinja?: JinjaScope;
	/** Checks the text (debounced); problems are underlined. */
	lint?: (text: string) => CodeDiagnostic[] | Promise<CodeDiagnostic[]>;
	onChange: (value: string) => void;
};
