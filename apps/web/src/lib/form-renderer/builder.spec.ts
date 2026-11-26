import { describe, expect, it } from 'vitest';
import {
	builderFieldsOf,
	definitionOf,
	nameFrom,
	problemsOf,
	type BuilderField
} from './builder.ts';
import { fieldsOf } from '@usehuxley/ui';

const field = (over: Partial<BuilderField>): BuilderField => ({
	name: 'x',
	label: '',
	kind: 'text',
	required: false,
	help: '',
	placeholder: '',
	options: [],
	showIf: null,
	lookup: null,
	extra: {},
	...over
});

describe('form builder', () => {
	it('round-trips a definition, keeping what it does not edit', () => {
		const schema = {
			type: 'object',
			required: ['upn'],
			properties: {
				upn: { type: 'string', format: 'email', title: 'User', minLength: 3 },
				notes: { type: 'string', description: 'Anything else' },
				licenses: {
					type: 'array',
					title: 'Licenses',
					items: { type: 'string', enum: ['E3', 'E5'] },
					uniqueItems: true
				},
				seats: { type: 'integer', minimum: 1 }
			}
		};
		const ui = {
			order: ['upn', 'licenses', 'notes', 'seats'],
			notes: { widget: 'textarea', placeholder: 'Optional' },
			legacy: { help: 'kept' }
		};
		const fields = builderFieldsOf(schema, ui);
		expect(fields.map((f) => [f.name, f.kind])).toEqual([
			['upn', 'email'],
			['licenses', 'multiselect'],
			['notes', 'textarea'],
			['seats', 'integer']
		]);
		const out = definitionOf(fields, ui);
		expect(out.schema.required).toEqual(['upn']);
		expect(out.schema.properties).toEqual({
			upn: { type: 'string', format: 'email', title: 'User', minLength: 3 },
			licenses: {
				type: 'array',
				title: 'Licenses',
				items: { type: 'string', enum: ['E3', 'E5'] },
				uniqueItems: true
			},
			notes: { type: 'string', description: 'Anything else' },
			seats: { type: 'integer', minimum: 1 }
		});
		expect(out.ui).toEqual({
			order: ['upn', 'licenses', 'notes', 'seats'],
			notes: { widget: 'textarea', placeholder: 'Optional' },
			legacy: { help: 'kept' }
		});
		// What renders is the same.
		expect(fieldsOf(out.schema, out.ui)).toEqual(fieldsOf(schema, ui));
	});

	it('changing a kind replaces what the old kind set', () => {
		const out = definitionOf([
			field({ name: 'team', kind: 'select', options: ['Red', ' ', 'Blue'], extra: { format: 'x' } })
		]);
		expect(out.schema.properties).toEqual({ team: { type: 'string', enum: ['Red', 'Blue'] } });
		const back = definitionOf([field({ name: 'team', kind: 'boolean', label: 'Team?' })]);
		expect(back.schema).toEqual({
			type: 'object',
			properties: { team: { type: 'boolean', title: 'Team?' } }
		});
	});

	it('keeps and edits show_if and options_from', () => {
		const schema = {
			type: 'object',
			properties: {
				kind: { type: 'string', enum: ['new', 'existing'] },
				ok: { type: 'boolean' },
				user: { type: 'string' },
				note: { type: 'string' }
			}
		};
		const ui = {
			user: {
				show_if: { field: 'kind', equals: 'existing' },
				options_from: { action: 'graph/listUsers', items: 'value', value: 'id', with: { top: 5 } },
				custom: 1
			},
			note: { show_if: { field: 'ok', in: [true] } }
		};
		const fields = builderFieldsOf(schema, ui);
		const user = fields.find((f) => f.name === 'user')!;
		expect(user.kind).toBe('select');
		expect(user.showIf).toEqual({ field: 'kind', values: 'existing' });
		expect(user.lookup?.action).toBe('graph/listUsers');
		const out = definitionOf(fields, ui);
		expect(out.schema.properties).toMatchObject({ user: { type: 'string' } });
		expect(out.ui.user).toEqual(ui.user);
		expect(out.ui.note).toEqual({ show_if: { field: 'ok', equals: true } });
		// Several values become `in`, typed for the field they test.
		user.showIf = { field: 'kind', values: 'new, existing' };
		expect(definitionOf(fields).ui.user).toMatchObject({
			show_if: { field: 'kind', in: ['new', 'existing'] }
		});
		user.lookup!.action = 'nope';
		expect(problemsOf(fields).get(fields.indexOf(user))).toMatch('integration');
	});

	it('secret fields are writeOnly strings', () => {
		const out = definitionOf([field({ name: 'pin', kind: 'secret' })]);
		expect(out.schema.properties).toEqual({ pin: { type: 'string', writeOnly: true } });
		expect(builderFieldsOf(out.schema, out.ui)[0].kind).toBe('secret');
		expect(
			definitionOf([field({ name: 'pin', kind: 'text', extra: { writeOnly: true } })]).schema
				.properties
		).toEqual({ pin: { type: 'string' } });
	});

	it('fields sharing a name (mid-edit) still render once', () => {
		const out = definitionOf([field({ name: 'a', label: 'One' }), field({ name: 'a' })]);
		expect(out.ui.order).toEqual(['a']);
		expect(fieldsOf(out.schema, { order: ['a', 'a'] }).map((f) => f.name)).toEqual(['a']);
	});

	it('names come from labels and problems are found', () => {
		expect(nameFrom('Start date', [])).toBe('start_date');
		expect(nameFrom('Start date', ['start_date'])).toBe('start_date_2');
		expect(nameFrom('2FA code', [])).toBe('f_2fa_code');
		expect(nameFrom('!!!', [])).toBe('field');
		const problems = problemsOf([
			field({ name: 'ok' }),
			field({ name: 'ok' }),
			field({ name: 'Bad Name' }),
			field({ name: 'pick', kind: 'select', options: [' '] })
		]);
		expect([...problems.keys()]).toEqual([1, 2, 3]);
	});
});
