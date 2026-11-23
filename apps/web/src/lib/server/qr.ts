import QRCode from 'qrcode';

/**
 * Renders `text` as an SVG QR code. Rendered on the server so the TOTP secret
 * never goes to a third-party QR service.
 */
export function qrSvg(text: string): Promise<string> {
	return QRCode.toString(text, { type: 'svg', errorCorrectionLevel: 'M', margin: 1 });
}
