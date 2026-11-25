import { formString } from '#lib/forms.ts';

/** An article's fields from its form. */
export function articleFields(form: FormData) {
	return {
		title: formString(form, 'title'),
		body: formString(form, 'body'),
		categoryId: formString(form, 'categoryId') || null,
		visibility: formString(form, 'visibility') || 'internal',
		tags: formString(form, 'tags')
			.split(',')
			.map((t) => t.trim())
			.filter(Boolean)
	};
}

/** What a status is called. */
export const STATUS_LABELS: Record<string, string> = {
	draft: 'Draft',
	in_review: 'In review',
	published: 'Published',
	archived: 'Archived'
};

export const statusTone = (status: string) =>
	status === 'published'
		? 'success'
		: status === 'in_review'
			? 'warning'
			: status === 'archived'
				? 'neutral'
				: 'info';
