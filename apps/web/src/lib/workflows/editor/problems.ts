/** Blocks that hold steps directly (a `[n]` after one of these is a step). */
const BLOCKS = new Set(['steps', 'then', 'else', 'default', 'catch', 'finally', 'on_error']);

/**
 * The step a diagnostic belongs to: its path up to the last step index.
 * `spec.steps[2].cases[0].steps[1].output` → `spec.steps[2].cases[0].steps[1]`;
 * `spec.steps[0].id` → `spec.steps[0]`; `metadata.key` → undefined.
 */
export function ownerOf(path: string): string | undefined {
	const pattern = /(?:^|\.)([a-z_]+)\[(\d+)\]/g;
	let owner: string | undefined;
	for (let match = pattern.exec(path); match; match = pattern.exec(path)) {
		if (BLOCKS.has(match[1])) owner = path.slice(0, match.index + match[0].length);
	}
	return owner;
}
