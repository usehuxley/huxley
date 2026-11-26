import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import {
	attempt,
	choose,
	defineInputs,
	expr,
	field,
	form,
	forEach,
	input,
	parallel,
	ref,
	step,
	text,
	trigger,
	when,
	workflow
} from '../src/index.ts';
import { build } from '../src/build.ts';
import { generate } from '../scripts/types.ts';

describe('references', () => {
	it('become expressions', () => {
		expect(String(ref.steps.fetch.output.body.items[0])).toBe(
			'{{ steps.fetch.output.body.items[0] }}'
		);
		expect(String(ref.trigger.headers['x-id'])).toBe('{{ trigger.headers["x-id"] }}');
		expect(text`Hi ${ref.inputs.name}!`).toBe('Hi {{ inputs.name }}!');
		expect(`Hi ${ref.inputs.name}!`).toBe('Hi {{ inputs.name }}!');
		expect(expr`${ref.vars.count} + ${2} > ${'three'}`).toBe('{{ vars.count + 2 > "three" }}');
		expect(step('t', 'transform', { output: ref.at('item').name })).toEqual({
			id: 't',
			type: 'transform',
			output: '{{ item.name }}'
		});
	});
});

describe('builders', () => {
	it('make control flow', () => {
		const say = (id: string) => step(id, 'transform', { output: id });
		expect(when('w', '{{ true }}', [say('a')])).toEqual({
			id: 'w',
			type: 'if',
			condition: '{{ true }}',
			then: [{ id: 'a', type: 'transform', output: 'a' }]
		});
		expect(choose('s', ref.inputs.kind, [['x', [say('x')]]], [say('d')])).toMatchObject({
			type: 'switch',
			value: '{{ inputs.kind }}',
			cases: [{ when: 'x', steps: [{ id: 'x' }] }],
			default: [{ id: 'd' }]
		});
		expect(forEach('f', [1, 2], [say('b')], { concurrency: 2 })).toMatchObject({
			items: [1, 2],
			concurrency: 2
		});
		expect(parallel('p', { left: [say('l')], right: [say('r')] }, { join: 'any' })).toMatchObject({
			branches: [{ id: 'left' }, { id: 'right' }],
			join: 'any'
		});
		expect(attempt('t', [say('a')], { catch: [say('c')] })).toMatchObject({ catch: [{ id: 'c' }] });
	});

	it('make inputs, triggers and workflows', () => {
		const inputs = defineInputs({
			name: input.string({ title: 'Name' }),
			count: input.integer({ default: 1 }).optional()
		});
		expect(inputs.schema).toEqual({
			type: 'object',
			properties: {
				name: { type: 'string', title: 'Name' },
				count: { type: 'integer', default: 1 }
			},
			required: ['name']
		});
		expect(String(inputs.refs.name)).toBe('{{ inputs.name }}');
		const wf = workflow({
			key: 'greet',
			name: 'Greet',
			triggers: [trigger.manual()],
			inputs: inputs.schema,
			steps: [step('say', 'transform', { output: text`Hello ${inputs.refs.name}` })],
			outputs: { line: ref.steps.say.output }
		});
		expect(wf.spec.outputs).toEqual({ line: '{{ steps.say.output }}' });
		expect(Object.keys(wf)).toEqual(['apiVersion', 'kind', 'metadata', 'spec']);
		expect('on_error' in wf.spec).toBe(false);
	});

	it('make forms', () => {
		const f = form({
			key: 'new-user',
			name: 'New user',
			fields: {
				name: field.text({ label: 'Their name', required: true }),
				laptop: field.yesNo({ label: 'Needs a laptop' }),
				model: field.choice(['13"', '15"'], {
					label: 'Model',
					showIf: { field: 'laptop', equals: [true] }
				})
			},
			portal: true
		});
		expect(f.schema.required).toEqual(['name']);
		expect(f.ui).toEqual({
			order: ['name', 'laptop', 'model'],
			model: { show_if: { field: 'laptop', equals: [true] } }
		});
		expect(f.portal).toBe(true);
	});
});

describe('build', () => {
	it('writes the same bytes every time, by key and folder', async () => {
		const src = await mkdtemp(join(tmpdir(), 'sdk-src-'));
		const sdk = new URL('../src/index.ts', import.meta.url).href;
		await writeFile(
			join(src, 'hello.workflow.ts'),
			`import { step, workflow } from '${sdk}';\nexport default workflow({ key: 'hello', name: 'Hello', steps: [step('a', 'transform', { output: 1 })] });\n`
		);
		await writeFile(
			join(src, 'ask.form.ts'),
			`import { field, form } from '${sdk}';\nexport default form({ key: 'ask', name: 'Ask', fields: { q: field.text({ label: 'Q' }) } });\n`
		);
		const [a, b] = [
			await mkdtemp(join(tmpdir(), 'sdk-a-')),
			await mkdtemp(join(tmpdir(), 'sdk-b-'))
		];
		const built = await build(src, a, join(a, '..', 'forms-a'));
		await build(src, b);
		expect(built).toEqual({ workflows: ['hello.json'], forms: ['ask.json'] });
		expect(await readFile(join(a, 'hello.json'), 'utf8')).toBe(
			await readFile(join(b, 'hello.json'), 'utf8')
		);
		await writeFile(
			join(src, 'again.workflow.ts'),
			await readFile(join(src, 'hello.workflow.ts'), 'utf8')
		);
		await expect(build(src, a)).rejects.toThrow(/the key hello is also used by/);
		for (const d of [src, a, b, join(a, '..', 'forms-a')])
			await rm(d, { recursive: true, force: true });
	});
});

describe('types', () => {
	it('are generated from the published schema', async () => {
		const schema = JSON.parse(
			await readFile(new URL('../../../schemas/hwf/v1.json', import.meta.url), 'utf8')
		) as Record<string, unknown>;
		const current = await readFile(new URL('../src/hwf.ts', import.meta.url), 'utf8');
		expect(current, 'src/hwf.ts is stale: run pnpm --filter @usehuxley/sdk generate').toBe(
			await generate(schema)
		);
	});
});
