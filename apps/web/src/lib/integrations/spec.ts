import { formString } from '#lib/forms.ts';

/** The spec from an uploaded file if there is one, else the pasted text. */
export async function specFrom(form: FormData): Promise<string> {
	const file = form.get('file');
	if (file instanceof File && file.size > 0) return await file.text();
	return formString(form, 'spec');
}
