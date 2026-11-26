/** A rating form's answer: the rating, and the comment if any. */
export function ratingFrom(form: FormData): { rating: number; comment?: string } | null {
	const raw = form.get('rating');
	const rating = typeof raw === 'string' ? Number(raw) : NaN;
	if (!Number.isInteger(rating) || rating < 1 || rating > 5) return null;
	const c = form.get('comment');
	const comment = typeof c === 'string' && c.trim() ? c.trim() : undefined;
	return { rating, comment };
}
