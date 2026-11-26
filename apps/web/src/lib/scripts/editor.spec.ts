import { describe, expect, it } from 'vitest';
import { editorLanguage, lintParameters, scriptWords } from './editor.ts';

const schema = {
	type: 'object',
	properties: { Computer: { type: 'string', description: 'Its name' } }
};

describe('script editor helpers', () => {
	it('maps languages', () => {
		expect(editorLanguage('pwsh')).toBe('powershell');
		expect(editorLanguage('python')).toBe('python');
		expect(editorLanguage('javascript')).toBe('javascript');
	});

	it('completes parameters the way each language receives them', () => {
		expect(scriptWords('pwsh', schema).map((w) => w.label)).toEqual(['$Computer']);
		expect(scriptWords('python', schema).map((w) => w.label)).toContain('inputs["Computer"]');
		expect(scriptWords('javascript', schema)[0]).toMatchObject({
			label: 'Computer',
			info: 'Its name'
		});
		expect(scriptWords('pwsh', 'nonsense')).toEqual([]);
	});

	it('finds where the parameters stop being JSON', () => {
		const text = '{\n  "type": "object",\n  "properties": { , }\n}';
		const [problem] = lintParameters(text);
		expect(problem.severity).toBe('error');
		expect(text.slice(problem.from, problem.from + 1)).toBe(',');
		expect(lintParameters('[1]')[0].message).toMatch(/object/);
		expect(lintParameters(JSON.stringify(schema))).toEqual([]);
	});
});
