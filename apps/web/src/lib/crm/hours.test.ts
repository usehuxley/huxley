import { describe, expect, it } from 'vitest';
import { hoursFromForm } from './hours';

describe('hoursFromForm', () => {
	it('keeps open days only', () => {
		const form = new FormData();
		form.set('open_mon', 'on');
		form.set('from_mon', '08:00');
		form.set('to_mon', '16:30');
		form.set('from_tue', '09:00');
		form.set('to_tue', '17:00');
		expect(hoursFromForm(form)).toEqual({ mon: [['08:00', '16:30']] });
	});
});
