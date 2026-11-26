/** The portal's look: brand name, accent color, logo. */
export type Branding = {
	name: string;
	accentColor: string;
	logo?: string | null;
	supportEmail?: string | null;
};

export const DEFAULT_BRANDING: Branding = { name: 'Support portal', accentColor: '#4f46e5' };

const HEX = /^#[0-9a-f]{6}$/i;

/** Text on the accent: near-black on light colors, white on dark ones. */
export function accentText(hex: string): string {
	const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
	const lin = (c: number) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
	const luminance = 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
	return luminance > 0.45 ? '#111827' : '#ffffff';
}

/** CSS variables that recolor the UI with the brand's accent. */
export function accentStyle(hex: string): string {
	if (!HEX.test(hex)) return '';
	return [
		`--hx-accent: ${hex}`,
		`--hx-accent-hover: color-mix(in srgb, ${hex} 85%, black)`,
		`--hx-accent-soft: color-mix(in srgb, ${hex} 14%, transparent)`,
		`--hx-accent-fg: ${accentText(hex)}`
	].join('; ');
}
