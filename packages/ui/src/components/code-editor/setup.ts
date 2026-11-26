// The editor itself (CodeMirror 6). Loaded only when a CodeEditor mounts,
// so pages without code never download it.
import {
	autocompletion,
	closeBrackets,
	closeBracketsKeymap,
	completionKeymap,
	type CompletionContext,
	type CompletionResult,
	type CompletionSource
} from '@codemirror/autocomplete';
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands';
import {
	bracketMatching,
	foldGutter,
	foldKeymap,
	HighlightStyle,
	indentOnInput,
	type LanguageSupport,
	StreamLanguage,
	syntaxHighlighting
} from '@codemirror/language';
import { linter, lintGutter, lintKeymap } from '@codemirror/lint';
import { highlightSelectionMatches, searchKeymap } from '@codemirror/search';
import { Compartment, EditorState, type Extension, type TransactionSpec } from '@codemirror/state';
import {
	drawSelection,
	EditorView,
	highlightActiveLine,
	highlightActiveLineGutter,
	keymap,
	lineNumbers,
	placeholder as placeholderText
} from '@codemirror/view';
import { tags as t } from '@lezer/highlight';
import type { CodeCompletion, CodeEditorOptions, CodeLanguage } from './types';

const v = (name: string) => `var(--hx-${name})`;

const theme = EditorView.theme({
	'&': {
		color: v('fg'),
		backgroundColor: v('surface'),
		fontSize: '0.8125rem',
		borderRadius: '0.5rem',
		border: `1px solid ${v('border-strong')}`
	},
	'&.cm-focused': { outline: `2px solid ${v('ring')}`, outlineOffset: '1px' },
	'.cm-scroller': { fontFamily: 'var(--font-mono)', lineHeight: '1.55' },
	'.cm-content': { caretColor: v('fg'), padding: '0.5rem 0' },
	'.cm-cursor, .cm-dropCursor': { borderLeftColor: v('fg') },
	'.cm-gutters': {
		backgroundColor: v('surface-sunken'),
		color: v('fg-subtle'),
		border: 'none',
		borderRadius: '0.5rem 0 0 0.5rem'
	},
	'.cm-activeLine': { backgroundColor: 'color-mix(in srgb, var(--hx-accent) 6%, transparent)' },
	'.cm-activeLineGutter': {
		backgroundColor: 'color-mix(in srgb, var(--hx-accent) 10%, transparent)'
	},
	'&.cm-focused .cm-selectionBackground, .cm-selectionBackground, .cm-content ::selection': {
		backgroundColor: 'color-mix(in srgb, var(--hx-accent) 25%, transparent)'
	},
	'.cm-matchingBracket': { backgroundColor: v('accent-soft'), outline: `1px solid ${v('accent')}` },
	'.cm-tooltip': {
		backgroundColor: v('surface-raised'),
		color: v('fg'),
		border: `1px solid ${v('border')}`,
		borderRadius: '0.5rem',
		boxShadow: v('shadow-lg')
	},
	'.cm-tooltip-autocomplete > ul > li[aria-selected]': {
		backgroundColor: v('accent-soft'),
		color: v('fg')
	},
	'.cm-completionDetail': { color: v('fg-muted'), fontStyle: 'normal', marginLeft: '0.5em' },
	'.cm-placeholder': { color: v('fg-subtle') },
	'.cm-panels': { backgroundColor: v('surface-sunken'), color: v('fg') },
	'.cm-diagnostic-error': { borderLeftColor: v('danger') },
	'.cm-diagnostic-warning': { borderLeftColor: v('warning') },
	'.cm-diagnostic-info': { borderLeftColor: v('info') }
});

const highlight = HighlightStyle.define([
	{
		tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword],
		color: v('code-keyword')
	},
	{ tag: [t.string, t.special(t.string), t.regexp], color: v('code-string') },
	{ tag: [t.number, t.bool, t.null, t.atom], color: v('code-number') },
	{
		tag: [t.comment, t.lineComment, t.blockComment],
		color: v('code-comment'),
		fontStyle: 'italic'
	},
	{ tag: [t.variableName, t.special(t.variableName)], color: v('code-variable') },
	{ tag: [t.function(t.variableName), t.function(t.propertyName)], color: v('code-function') },
	{ tag: [t.typeName, t.className, t.namespace], color: v('code-type') },
	{ tag: [t.propertyName, t.attributeName], color: v('code-variable') },
	{ tag: [t.meta, t.brace, t.processingInstruction, t.tagName], color: v('code-meta') },
	{ tag: t.invalid, color: v('danger') }
]);

/** Loads a language's support, once. */
const cache = new Map<string, Promise<Extension>>();
function load(language: CodeLanguage, options: CodeEditorOptions): Promise<Extension> {
	// Jinja's completion depends on the editor's scope: never shared.
	if (language === 'jinja') {
		return import('@codemirror/lang-jinja').then(({ jinja }) =>
			jinja({
				variables: options.jinja?.variables?.map(completion),
				properties: options.jinja?.properties
					? (path) => (options.jinja?.properties?.(path) ?? []).map(completion)
					: undefined
			})
		);
	}
	let found = cache.get(language);
	if (!found) {
		const lang = (p: Promise<LanguageSupport>): Promise<Extension> => p;
		found = {
			powershell: () =>
				import('@codemirror/legacy-modes/mode/powershell').then(({ powerShell }) =>
					StreamLanguage.define(powerShell)
				),
			python: () => lang(import('@codemirror/lang-python').then((m) => m.python())),
			javascript: () => lang(import('@codemirror/lang-javascript').then((m) => m.javascript())),
			typescript: () =>
				lang(import('@codemirror/lang-javascript').then((m) => m.javascript({ typescript: true }))),
			yaml: () => lang(import('@codemirror/lang-yaml').then((m) => m.yaml())),
			json: () => lang(import('@codemirror/lang-json').then((m) => m.json())),
			markdown: () => lang(import('@codemirror/lang-markdown').then((m) => m.markdown())),
			text: () => Promise.resolve([] as Extension)
		}[language]();
		cache.set(language, found);
	}
	return found;
}

function completion(c: CodeCompletion) {
	return { label: c.label, detail: c.detail, info: c.info, type: c.type ?? 'variable' };
}

/** Completes the given words (e.g. `$Name` for script parameters). */
function wordSource(words: () => CodeCompletion[]): CompletionSource {
	return (context: CompletionContext): CompletionResult | null => {
		const word = context.matchBefore(/[$\w][\w.-]*/);
		if (!word || (word.from === word.to && !context.explicit)) return null;
		// Filtered here, case-insensitively: CodeMirror's own matching passes
		// over words starting with `$` like PowerShell's.
		const typed = word.text.toLowerCase();
		const options = words()
			.filter((w) => w.label.toLowerCase().includes(typed))
			.map(completion);
		if (options.length === 0) return null;
		return { from: word.from, options, filter: false };
	};
}

/** Completes with the page's own logic (it filters, so CodeMirror doesn't). */
function pageSource(complete: NonNullable<CodeEditorOptions['complete']>): CompletionSource {
	return (context: CompletionContext): CompletionResult | null => {
		const found = complete(context.state.doc.toString(), context.pos);
		if (!found || found.options.length === 0) return null;
		return {
			from: found.from,
			to: found.to,
			options: found.options.map(completion),
			filter: false
		};
	};
}

/** Browsers put non-breaking spaces into editable text where a typed space
 * would otherwise collapse (beside closed brackets, say). In code they're
 * always a mistake (an expression with one doesn't parse), so typed and
 * pasted ones become ordinary spaces. */
const plainSpaces = EditorState.transactionFilter.of((tr) => {
	if (!tr.docChanged || !tr.isUserEvent('input')) return tr;
	let found = false;
	tr.changes.iterChanges((_fa, _ta, _fb, _tb, inserted) => {
		if (inserted.toString().includes('\u00a0')) found = true;
	});
	if (!found) return tr;
	const changes: { from: number; to: number; insert: string }[] = [];
	tr.changes.iterChanges((from, to, _fb, _tb, inserted) => {
		changes.push({ from, to, insert: inserted.toString().replaceAll('\u00a0', ' ') });
	});
	// Same lengths, so the selection and effects carry over unchanged.
	const spec: TransactionSpec = {
		changes,
		selection: tr.selection,
		effects: tr.effects,
		scrollIntoView: tr.scrollIntoView,
		userEvent: 'input'
	};
	return spec;
});

export type Controller = {
	setValue: (value: string) => void;
	setReadonly: (readonly: boolean) => void;
	focus: () => void;
	destroy: () => void;
};

export async function create(parent: HTMLElement, options: CodeEditorOptions): Promise<Controller> {
	const language = new Compartment();
	const editable = new Compartment();
	const lint = options.lint;
	const gutters: Extension[] = options.compact
		? []
		: [lineNumbers(), highlightActiveLineGutter(), foldGutter(), highlightActiveLine()];
	const extensions: Extension[] = [
		...gutters,
		history(),
		drawSelection(),
		indentOnInput(),
		bracketMatching(),
		closeBrackets(),
		plainSpaces,
		highlightSelectionMatches(),
		syntaxHighlighting(highlight),
		theme,
		EditorView.lineWrapping,
		EditorView.contentAttributes.of({ 'aria-label': options.label }),
		// Tab indents; Escape then Tab leaves the editor (CodeMirror's own rule).
		keymap.of([
			...closeBracketsKeymap,
			...defaultKeymap,
			...searchKeymap,
			...historyKeymap,
			...foldKeymap,
			...completionKeymap,
			...lintKeymap,
			indentWithTab
		]),
		language.of(await load(options.language, options)),
		editable.of([
			EditorState.readOnly.of(options.readonly),
			EditorView.editable.of(!options.readonly)
		]),
		EditorView.updateListener.of((u) => {
			if (u.docChanged) options.onChange(u.state.doc.toString());
		}),
		EditorView.domEventHandlers({
			blur: () => {
				options.onBlur?.();
			}
		})
	];
	if (options.placeholder) extensions.push(placeholderText(options.placeholder));
	// Each language's own completion, plus the given words alongside it.
	extensions.push(autocompletion());
	const words = options.words;
	const complete = options.complete;
	if (complete) {
		const data = [{ autocomplete: pageSource(complete) }];
		extensions.push(EditorState.languageData.of(() => data));
	}
	if (words) {
		// One source for the editor's life: CodeMirror matches results to
		// sources by identity, so a new function per lookup drops them all.
		const source = wordSource(words);
		const data = [{ autocomplete: source }];
		extensions.push(EditorState.languageData.of(() => data));
	}
	if (lint) {
		extensions.push(
			lintGutter(),
			linter(async (view) => lint(view.state.doc.toString()), { delay: 500 })
		);
	}
	const view = new EditorView({
		parent,
		state: EditorState.create({ doc: options.value, extensions })
	});
	return {
		setValue(value) {
			const current = view.state.doc.toString();
			if (value !== current) {
				view.dispatch({ changes: { from: 0, to: current.length, insert: value } });
			}
		},
		setReadonly(readonly) {
			view.dispatch({
				effects: editable.reconfigure([
					EditorState.readOnly.of(readonly),
					EditorView.editable.of(!readonly)
				])
			});
		},
		focus: () => view.focus(),
		destroy: () => view.destroy()
	};
}
