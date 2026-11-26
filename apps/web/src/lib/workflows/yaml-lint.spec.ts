import { describe, expect, it } from 'vitest';
import { lintJson, lintWorkflowYaml, pathParts, rangeOf, syntaxProblems } from './yaml-lint.ts';

const yaml = `apiVersion: usehuxley.io/v1
kind: Workflow
spec:
  steps:
    - id: greet
      type: transform
      output: "{{ steps.later.output }}"
`;

describe('workflow YAML problems', () => {
	it('splits paths', () => {
		expect(pathParts('spec.steps[0].with.x')).toEqual(['spec', 'steps', 0, 'with', 'x']);
		expect(pathParts('spec.steps[1].cases[0].steps[2]')).toEqual([
			'spec',
			'steps',
			1,
			'cases',
			0,
			'steps',
			2
		]);
		expect(pathParts('metadata.key')).toEqual(['metadata', 'key']);
	});

	it('finds a path in the source, or its nearest parent', () => {
		const at = rangeOf(yaml, 'spec.steps[0].output');
		expect(yaml.slice(at.from, at.to)).toBe('"{{ steps.later.output }}"');
		const missing = rangeOf(yaml, 'spec.steps[0].with.ticket_id');
		expect(yaml.slice(missing.from, missing.from + 9)).toBe('id: greet');
	});

	it('reports syntax errors where they are', () => {
		const [problem] = syntaxProblems('spec:\n  steps: [\n');
		expect(problem.severity).toBe('error');
		expect(syntaxProblems(yaml)).toEqual([]);
	});

	it("underlines the server's problems at their paths", async () => {
		const problems = await lintWorkflowYaml(yaml, () =>
			Promise.resolve([{ path: 'spec.steps[0].output', message: 'no step later has run' }])
		);
		expect(problems).toHaveLength(1);
		expect(yaml.slice(problems[0].from, problems[0].to)).toContain('steps.later');
		expect(problems[0].message).toBe('spec.steps[0].output: no step later has run');
	});

	it('checks JSON', () => {
		expect(lintJson('{"a": 1}')).toEqual([]);
		const [problem] = lintJson('{"a": }');
		expect(problem.severity).toBe('error');
	});
});
