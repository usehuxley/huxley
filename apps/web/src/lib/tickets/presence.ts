export type Viewer = { userId: string; name: string; typing: boolean };

/** "Ada is viewing", "Ada and Ben are viewing; Ben is writing". Empty: alone. */
export function describePresence(viewers: Viewer[]): string {
	if (viewers.length === 0) return '';
	const names = (list: Viewer[]) =>
		list.length === 1
			? list[0].name
			: `${list
					.slice(0, -1)
					.map((v) => v.name)
					.join(', ')} and ${list.at(-1)!.name}`;
	const writing = viewers.filter((v) => v.typing);
	const watching = viewers.filter((v) => !v.typing);
	const parts = [];
	if (writing.length > 0)
		parts.push(`${names(writing)} ${writing.length === 1 ? 'is' : 'are'} writing`);
	if (watching.length > 0)
		parts.push(`${names(watching)} ${watching.length === 1 ? 'is' : 'are'} also viewing`);
	return parts.join('; ');
}
