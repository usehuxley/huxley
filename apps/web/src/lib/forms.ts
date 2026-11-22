/** A text field from submitted form data. File uploads and missing fields read as ''. */
export function formString(form: FormData, name: string): string {
	const value = form.get(name);
	return typeof value === 'string' ? value : '';
}
