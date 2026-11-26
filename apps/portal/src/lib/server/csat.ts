/** A rating form's answer: the rating, and the comment if any. */
export function ratingFrom(form: FormData): { rating: number; comment?: string } | null {
	const raw = form.get('rating');
	const rating = typeof raw === 'string' ? Number(raw) : NaN;
	if (!Number.isInteger(rating) || rating < 1 || rating > 5) return null;
	const c = form.get('comment');
	const comment = typeof c === 'string' && c.trim() ? c.trim() : undefined;
	return { rating, comment };
}

/** An NPS form's answer: the score (0 to 10), and the comment if any. */
export function scoreFrom(form: FormData): { score: number; comment?: string } | null {
	const raw = form.get('score');
	const score = typeof raw === 'string' && raw !== '' ? Number(raw) : NaN;
	if (!Number.isInteger(score) || score < 0 || score > 10) return null;
	const c = form.get('comment');
	const comment = typeof c === 'string' && c.trim() ? c.trim() : undefined;
	return { score, comment };
}
