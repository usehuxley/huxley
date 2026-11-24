import { formString } from '#lib/forms.ts';
import { DAYS } from './labels.ts';

/** Hours from `HoursFields`: `{"mon": [["09:00", "17:00"]], …}`, open days only. */
export function hoursFromForm(form: FormData): Record<string, string[][]> {
	const hours: Record<string, string[][]> = {};
	for (const { key } of DAYS) {
		if (form.get(`open_${key}`) !== 'on') continue;
		hours[key] = [[formString(form, `from_${key}`), formString(form, `to_${key}`)]];
	}
	return hours;
}
