/** A survey's choices for its scale: `five` (1 to 5) or `thumbs` (1 down, 5 up). */
export function choices(scale: string): { value: number; label: string }[] {
	return scale === 'thumbs'
		? [
				{ value: 5, label: '👍 Good' },
				{ value: 1, label: '👎 Not good' }
			]
		: [
				{ value: 5, label: '5 · Great' },
				{ value: 4, label: '4 · Good' },
				{ value: 3, label: '3 · OK' },
				{ value: 2, label: '2 · Poor' },
				{ value: 1, label: '1 · Bad' }
			];
}

/** An answer as words. */
export function ratingText(scale: string, rating: number): string {
	return choices(scale).find((c) => c.value === rating)?.label ?? `${rating}`;
}
