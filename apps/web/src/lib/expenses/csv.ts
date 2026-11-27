/** Splits CSV text into rows of cells (quotes, doubled quotes, CRLF). */
export function parseCsv(text: string): string[][] {
	const rows: string[][] = [];
	let row: string[] = [];
	let cell = '';
	let quoted = false;
	for (let i = 0; i < text.length; i++) {
		const c = text[i];
		if (quoted) {
			if (c === '"' && text[i + 1] === '"') {
				cell += '"';
				i++;
			} else if (c === '"') {
				quoted = false;
			} else {
				cell += c;
			}
		} else if (c === '"') {
			quoted = true;
		} else if (c === ',') {
			row.push(cell);
			cell = '';
		} else if (c === '\n' || c === '\r') {
			if (c === '\r' && text[i + 1] === '\n') i++;
			row.push(cell);
			if (row.some((v) => v.trim() !== '')) rows.push(row);
			row = [];
			cell = '';
		} else {
			cell += c;
		}
	}
	row.push(cell);
	if (row.some((v) => v.trim() !== '')) rows.push(row);
	return rows;
}

/** A day as YYYY-MM-DD, from YYYY-MM-DD or M/D/YYYY. */
export function day(value: string): string | null {
	const v = value.trim();
	if (/^\d{4}-\d{2}-\d{2}$/.test(v)) return v;
	const m = /^(\d{1,2})\/(\d{1,2})\/(\d{4})$/.exec(v);
	if (!m) return null;
	return `${m[3]}-${m[1].padStart(2, '0')}-${m[2].padStart(2, '0')}`;
}
