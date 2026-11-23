// Test-only helpers for end-to-end tests (Node). Not imported by the app.
import { createHmac } from 'node:crypto';

export function base32Decode(input: string): Buffer {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZ234567';
	let bits = 0;
	let value = 0;
	const out: number[] = [];
	for (const char of input.replace(/\s/g, '')) {
		value = (value << 5) | alphabet.indexOf(char);
		bits += 5;
		if (bits >= 8) {
			bits -= 8;
			out.push((value >>> bits) & 0xff);
		}
	}
	return Buffer.from(out);
}

/** RFC 6238 code for the current 30-second step plus `offset` steps. */
export function totp(secret: Buffer, offset = 0): string {
	const counter = Buffer.alloc(8);
	counter.writeBigInt64BE(BigInt(Math.floor(Date.now() / 30000) + offset));
	const digest = createHmac('sha1', secret).update(counter).digest();
	const start = digest[digest.length - 1] & 0x0f;
	const binary = digest.readUInt32BE(start) & 0x7fffffff;
	return String(binary % 1_000_000).padStart(6, '0');
}
