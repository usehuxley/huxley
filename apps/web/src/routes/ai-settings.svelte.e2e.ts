import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { expect, test, type Browser, type Locator, type Page } from '@playwright/test';

const ownerEmail = process.env.E2E_EMAIL;
const ownerPassword = process.env.E2E_PASSWORD;

async function signIn(browser: Browser): Promise<Page> {
	const page = await (await browser.newContext()).newPage();
	await page.goto('/login');
	await page.getByLabel('Email').fill(ownerEmail!);
	await page.getByLabel('Password').fill(ownerPassword!);
	await page.getByRole('button', { name: 'Sign in', exact: true }).click();
	await expect(page.getByRole('heading', { level: 1 })).toHaveText('Welcome to Huxley');
	return page;
}

// These share the organization's one AI provider, so they run in order.
/**
 * Chooses the OpenAI-compatible provider. Its Base URL field appears once
 * the page reacts to the choice, and a choice made before it's ready is
 * lost, so choose until the field shows.
 */
async function chooseCompatible(card: Locator) {
	await expect(async () => {
		await card.getByLabel('Provider', { exact: true }).selectOption('openai_compatible');
		await expect(card.getByLabel('Base URL')).toBeVisible({ timeout: 1000 });
	}).toPass({ timeout: 15_000 });
}

/** The AI settings page's card for one provider. */
const aiCard = (page: Page) => page.getByRole('region', { name: 'AI provider' });
const embeddingsCard = (page: Page) => page.getByRole('region', { name: 'Embeddings provider' });

test.describe('AI settings and the assistant', () => {
	test.describe.configure({ mode: 'serial' });
	test.skip(!ownerEmail || !ownerPassword, 'set E2E_EMAIL and E2E_PASSWORD');

	test('without an AI provider, the assistant says how to set one up', async ({ browser }) => {
		const page = await signIn(browser);
		await page.getByRole('link', { name: 'Assistant' }).click();
		await expect(page.getByRole('heading', { level: 1 })).toHaveText('Assistant');
		await page.getByLabel('Message').fill('List my workflows');
		await page.getByRole('button', { name: 'Ask' }).click();
		await expect(page.getByRole('alert')).toContainText('Set up an AI provider first');
		await expect(page.getByRole('list', { name: 'Conversations' })).not.toContainText(
			'List my workflows'
		);
	});

	test('a provider is set, tested, logged and removed', async ({ browser }) => {
		const page = await signIn(browser);
		await page.goto('/settings/ai');
		const card = aiCard(page);
		await chooseCompatible(card);
		await card.getByLabel('Model').fill('local-model');
		// Nothing answers at a .invalid host: the test fails without reaching any real service.
		await card.getByLabel('Base URL').fill('https://ai.example.invalid/v1');
		await card.getByLabel('API key').fill('sk-e2e-not-real');
		await card.getByRole('button', { name: 'Save' }).click();
		await expect(page.getByText('AI provider saved.')).toBeVisible();
		await expect(page.getByTestId('chat-effective')).toContainText('OpenAI-compatible');
		await expect(page.getByTestId('chat-effective')).toContainText('local-model');
		// The key is never shown again.
		await expect(card.getByLabel('API key')).toHaveValue('');
		await expect(page.locator('body')).not.toContainText('sk-e2e-not-real');

		await card.getByRole('button', { name: 'Test' }).click();
		await expect(page.getByTestId('chat-test')).toContainText("couldn't reach the provider");
		await page.reload();
		await expect(page.getByRole('list', { name: 'AI calls' })).toContainText('provider_test');
		await expect(page.getByRole('list', { name: 'AI calls' })).toContainText('failed');

		// The assistant reports a provider it can't reach, plainly.
		await page.goto('/assistant');
		await page.getByLabel('Message').fill('Hello there');
		await page.getByRole('button', { name: 'Ask' }).click();
		await expect(page.getByRole('alert')).toContainText("couldn't reach the provider");

		await page.goto('/settings/ai');
		await aiCard(page).getByRole('button', { name: 'Remove' }).click();
		await expect(page.getByText('AI provider removed.')).toBeVisible();
		await expect(page.getByText('No AI provider')).toBeVisible();
	});

	test('answers stream in as the model writes them', async ({ browser }) => {
		// An OpenAI-compatible model that writes slowly, word by word. The
		// e2e server allows calls to 127.0.0.1 (engine.egress.private_hosts).
		const words = ['Streaming ', 'answers ', 'arrive ', 'word ', 'by ', 'word.'];
		let release!: () => void;
		const held = new Promise<void>((resolve) => (release = resolve));
		const model: Server = createServer((req, res) => {
			let body = '';
			req.on('data', (chunk: Buffer) => (body += chunk.toString()));
			req.on('end', () => {
				void (async () => {
					const streamed = (JSON.parse(body) as { stream?: boolean }).stream === true;
					if (!streamed) {
						res.setHeader('content-type', 'application/json');
						res.end(
							JSON.stringify({
								model: 'mock',
								choices: [{ message: { content: words.join('') }, finish_reason: 'stop' }],
								usage: { prompt_tokens: 1, completion_tokens: 1 }
							})
						);
						return;
					}
					res.setHeader('content-type', 'text/event-stream');
					const send = (data: unknown) => res.write(`data: ${JSON.stringify(data)}\n\n`);
					for (const [i, word] of words.entries()) {
						send({ model: 'mock', choices: [{ index: 0, delta: { content: word } }] });
						// Hold the rest back until the test has seen the first part.
						if (i === 1) await held;
					}
					send({ model: 'mock', choices: [{ index: 0, delta: {}, finish_reason: 'stop' }] });
					res.end('data: [DONE]\n\n');
				})();
			});
		});
		await new Promise<void>((resolve) => model.listen(0, '127.0.0.1', resolve));
		const { port } = model.address() as AddressInfo;
		try {
			const page = await signIn(browser);
			await page.goto('/settings/ai');
			const card = aiCard(page);
			await chooseCompatible(card);
			await card.getByLabel('Model').fill('mock');
			await card.getByLabel('Base URL').fill(`http://127.0.0.1:${port}/v1`);
			await card.getByLabel('API key').fill('sk-e2e-mock');
			await card.getByRole('button', { name: 'Save' }).click();
			await expect(page.getByText('AI provider saved.')).toBeVisible();

			await page.goto('/assistant');
			await page.getByLabel('Message').fill('Show me streaming');
			await page.getByRole('button', { name: 'Ask' }).click();
			// The first words are on the page while the model is still writing.
			const answering = page.getByRole('list', { name: 'Answer in progress' });
			await expect(answering).toContainText('Show me streaming');
			await expect(answering).toContainText('Streaming answers');
			await expect(answering).not.toContainText('word.');
			release();
			// Then the saved conversation replaces it.
			const messages = page.getByRole('list', { name: 'Messages' });
			await expect(messages).toContainText('Streaming answers arrive word by word.');
			await expect(answering).toHaveCount(0);
			await expect(page).toHaveURL(/\/assistant\?c=/);

			await page.goto('/settings/ai');
			await aiCard(page).getByRole('button', { name: 'Remove' }).click();
			await expect(page.getByText('AI provider removed.')).toBeVisible();
		} finally {
			release();
			model.close();
		}
	});

	test('an embeddings provider is set apart from the AI provider', async ({ browser }) => {
		// An OpenAI-compatible embeddings API: three numbers per text.
		const model = createServer((req, res) => {
			let body = '';
			req.on('data', (chunk) => (body += chunk));
			req.on('end', () => {
				const { input } = JSON.parse(body) as { input: string[] };
				res.writeHead(200, { 'content-type': 'application/json' });
				res.end(
					JSON.stringify({
						model: 'mock-embed',
						data: input.map((_, index) => ({ index, embedding: [0.1, 0.2, 0.3] })),
						usage: { prompt_tokens: 1, total_tokens: 1 }
					})
				);
			});
		});
		await new Promise<void>((resolve) => model.listen(0, '127.0.0.1', resolve));
		const { port } = model.address() as AddressInfo;
		try {
			const page = await signIn(browser);
			await page.goto('/settings/ai');
			await expect(page.getByText('No embeddings provider')).toBeVisible();
			const card = embeddingsCard(page);
			// Anthropic makes no embeddings, so it isn't offered; Voyage is.
			const kinds = card.getByLabel('Provider', { exact: true });
			await expect(kinds.locator('option[value="anthropic"]')).toHaveCount(0);
			await expect(kinds.locator('option[value="voyage"]')).toHaveCount(1);
			await chooseCompatible(card);
			await card.getByLabel('Model').fill('mock-embed');
			await card.getByLabel('Base URL').fill(`http://127.0.0.1:${port}/v1`);
			await card.getByLabel('API key').fill('sk-e2e-embed');
			await card.getByRole('button', { name: 'Save' }).click();
			await expect(page.getByText('Embeddings provider saved.')).toBeVisible();
			await expect(page.getByTestId('embeddings-effective')).toContainText('mock-embed');
			// The AI provider is untouched.
			await expect(page.getByText('No AI provider')).toBeVisible();

			await card.getByRole('button', { name: 'Test' }).click();
			await expect(page.getByTestId('embeddings-test')).toContainText(
				'Working: mock-embed gave 3 dimensions'
			);

			await embeddingsCard(page).getByRole('button', { name: 'Remove' }).click();
			await expect(page.getByText('Embeddings provider removed.')).toBeVisible();
			await expect(page.getByText('No embeddings provider')).toBeVisible();
		} finally {
			model.close();
		}
	});
});
