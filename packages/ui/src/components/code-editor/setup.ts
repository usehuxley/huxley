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
import { Compartment, EditorState, type Extension } from '@codemirror/state';
import {
	drawSelection,
	EditorView,
	highlightActiveLine,
	highlightActiveLineGutter,
	keymap,
	lineNumbers,
	placeholder as placeholderText,
	tooltips
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
	const extensions: Extension[] = [
		lineNumbers(),
		highlightActiveLineGutter(),
		foldGutter(),
		history(),
		drawSelection(),
		indentOnInput(),
		bracketMatching(),
		closeBrackets(),
		highlightActiveLine(),
		highlightSelectionMatches(),
		syntaxHighlighting(highlight),
		theme,
		// Completion lists and lint messages on the page itself, so a card or
		// panel that clips its overflow can't cut them off.
		tooltips({ parent: document.body }),
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
		})
	];
	if (options.placeholder) extensions.push(placeholderText(options.placeholder));
	// CodeMirror adds its styles as a <style> tag, which a strict CSP allows
	// only with the page's nonce.
	if (options.cspNonce) extensions.push(EditorView.cspNonce.of(options.cspNonce));
	// Each language's own completion, plus the given words alongside it.
	extensions.push(autocompletion());
	const words = options.words;
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
