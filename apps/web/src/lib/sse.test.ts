import { describe, expect, it } from 'vitest';
import { SseReader, readEvents } from './sse.ts';

describe('SseReader', () => {
	it('finds events however the text is split', () => {
		const wire =
			'event: text\ndata: {"text":"Hel"}\n\nevent: text\r\ndata: {"text":"lo"}\r\n\r\n: keep-alive\n\nevent: done\ndata: {"conversation":{"id":"c1"}}\n\n';
		for (const size of [1, 2, 3, 7, wire.length]) {
			const reader = new SseReader();
			const events = [];
			for (let i = 0; i < wire.length; i += size)
				events.push(...reader.push(wire.slice(i, i + size)));
			expect(events).toEqual([
				{ event: 'text', data: { text: 'Hel' } },
				{ event: 'text', data: { text: 'lo' } },
				{ event: 'done', data: { conversation: { id: 'c1' } } }
			]);
		}
	});
});

describe('readEvents', () => {
	it('reads a response body, multi-byte characters split across chunks included', async () => {
		const bytes = new TextEncoder().encode('event: text\ndata: {"text":"été 😀"}\n\n');
		const body = new ReadableStream<Uint8Array>({
			start(controller) {
				for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
				controller.close();
			}
		});
		const events = [];
		for await (const event of readEvents(body)) events.push(event);
		expect(events).toEqual([{ event: 'text', data: { text: 'été 😀' } }]);
	});
});
