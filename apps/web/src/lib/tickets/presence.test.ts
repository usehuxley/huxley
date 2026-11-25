import { describe, expect, it } from 'vitest';
import { describePresence } from './presence';

const v = (name: string, typing = false) => ({ userId: name, name, typing });

describe('describePresence', () => {
	it('says nothing when you are alone', () => {
		expect(describePresence([])).toBe('');
	});

	it('names who is viewing and who is writing', () => {
		expect(describePresence([v('Ada')])).toBe('Ada is also viewing');
		expect(describePresence([v('Ben', true)])).toBe('Ben is writing');
		expect(describePresence([v('Ben', true), v('Ada'), v('Cy')])).toBe(
			'Ben is writing; Ada and Cy are also viewing'
		);
		expect(describePresence([v('Ada', true), v('Ben', true), v('Cy', true)])).toBe(
			'Ada, Ben and Cy are writing'
		);
	});
});
