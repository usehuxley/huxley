import { describe, expect, it } from 'vitest';
import { answersOf, fieldsOf } from '@usehuxley/ui';

const schema = {
	type: 'object',
	required: ['name'],
	properties: {
		name: { type: 'string', title: 'Full name' },
		notes: { type: 'string' },
		plan: { type: 'string', enum: ['E3', 'E5'] },
		start: { type: 'string', format: 'date' },
		seats: { type: 'integer' },
		apps: { type: 'array', items: { type: 'string', enum: ['Teams', 'Visio'] } },
		manager: { type: 'boolean' }
	}
};

describe('form renderer fields', () => {
	it('maps schema properties to fields in the hinted order', () => {
		const fields = fieldsOf(schema, {
			order: ['seats'],
			notes: { widget: 'textarea', help: 'Anything else' }
		});
		expect(fields.map((f) => [f.name, f.kind])).toEqual([
			['seats', 'integer'],
			['name', 'text'],
			['notes', 'textarea'],
			['plan', 'select'],
			['start', 'date'],
			['apps', 'multiselect'],
			['manager', 'boolean']
		]);
		expect(fields[1]).toMatchObject({ label: 'Full name', required: true });
		expect(fields[2].help).toBe('Anything else');
		expect(fields[5].options).toEqual(['Teams', 'Visio']);
	});

	it('types answers and leaves out empty ones', () => {
		const fields = fieldsOf(schema, {});
		expect(
			answersOf(fields, { name: 'Ada', notes: '', seats: '3', apps: [], manager: false })
		).toEqual({
			name: 'Ada',
			seats: 3,
			manager: false
		});
	});
});
