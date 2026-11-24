/** One Server-Sent Event: its name and its data, parsed as JSON. */
export interface SseMessage {
	event: string;
	data: unknown;
}

/**
 * Splits Server-Sent Events out of text that arrives in pieces. Feed it
 * text; it returns the events completed so far.
 */
export class SseReader {
	#buffer = '';

	push(text: string): SseMessage[] {
		this.#buffer += text;
		// A trailing "\r" may be half of a "\r\n" still on its way.
		const held = this.#buffer.endsWith('\r') ? '\r' : '';
		let ready = held ? this.#buffer.slice(0, -1) : this.#buffer;
		ready = ready.replaceAll('\r\n', '\n').replaceAll('\r', '\n');
		const messages: SseMessage[] = [];
		let end: number;
		while ((end = ready.indexOf('\n\n')) !== -1) {
			const raw = ready.slice(0, end);
			ready = ready.slice(end + 2);
			let event = 'message';
			const data: string[] = [];
			for (const line of raw.split('\n')) {
				if (line.startsWith('event:')) event = line.slice(6).trim();
				else if (line.startsWith('data:')) data.push(line.slice(5).replace(/^ /, ''));
			}
			if (data.length === 0) continue;
			try {
				messages.push({ event, data: JSON.parse(data.join('\n')) as unknown });
			} catch {
				messages.push({ event, data: data.join('\n') });
			}
		}
		this.#buffer = ready + held;
		return messages;
	}
}

/** Reads a streamed response body as events. */
export async function* readEvents(body: ReadableStream<Uint8Array>): AsyncGenerator<SseMessage> {
	const reader = new SseReader();
	const decoder = new TextDecoder();
	for await (const chunk of body) {
		yield* reader.push(decoder.decode(chunk, { stream: true }));
	}
	yield* reader.push(decoder.decode() + '\n\n');
}
