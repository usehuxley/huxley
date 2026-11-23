/** One line of a diff: kept, added, or removed. */
export interface DiffLine {
	kind: ' ' | '+' | '-';
	text: string;
}

/** Longest inputs compared line by line; beyond this the diff is skipped. */
export const MAX_DIFF_LINES = 5000;

/**
 * A line diff of `before` → `after` (longest common subsequence). Returns
 * `null` when either side is too long to compare.
 */
export function diffLines(before: string, after: string): DiffLine[] | null {
	const a = before.split('\n');
	const b = after.split('\n');
	if (a.length > MAX_DIFF_LINES || b.length > MAX_DIFF_LINES) return null;
	// Trim the common head and tail; most edits are small.
	let start = 0;
	while (start < a.length && start < b.length && a[start] === b[start]) start++;
	let endA = a.length;
	let endB = b.length;
	while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
		endA--;
		endB--;
	}
	const midA = a.slice(start, endA);
	const midB = b.slice(start, endB);
	// LCS lengths over the changed middle.
	const rows = midA.length + 1;
	const cols = midB.length + 1;
	const lcs = new Uint32Array(rows * cols);
	for (let i = midA.length - 1; i >= 0; i--) {
		for (let j = midB.length - 1; j >= 0; j--) {
			lcs[i * cols + j] =
				midA[i] === midB[j]
					? lcs[(i + 1) * cols + j + 1] + 1
					: Math.max(lcs[(i + 1) * cols + j], lcs[i * cols + j + 1]);
		}
	}
	const result: DiffLine[] = a.slice(0, start).map((text) => ({ kind: ' ', text }));
	let i = 0;
	let j = 0;
	while (i < midA.length && j < midB.length) {
		if (midA[i] === midB[j]) {
			result.push({ kind: ' ', text: midA[i] });
			i++;
			j++;
		} else if (lcs[(i + 1) * cols + j] >= lcs[i * cols + j + 1]) {
			result.push({ kind: '-', text: midA[i++] });
		} else {
			result.push({ kind: '+', text: midB[j++] });
		}
	}
	while (i < midA.length) result.push({ kind: '-', text: midA[i++] });
	while (j < midB.length) result.push({ kind: '+', text: midB[j++] });
	for (const text of a.slice(endA)) result.push({ kind: ' ', text });
	return result;
}
