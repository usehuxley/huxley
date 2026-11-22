import { page } from 'vitest/browser';
import { describe, expect, it } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { EmptyState } from '@usehuxley/ui';

describe('EmptyState', () => {
	it('renders the title and description', async () => {
		await render(EmptyState, {
			title: 'No tickets yet',
			description: 'Tickets you create appear here.'
		});

		await expect.element(page.getByRole('heading', { name: 'No tickets yet' })).toBeInTheDocument();
		await expect.element(page.getByText('Tickets you create appear here.')).toBeInTheDocument();
	});
});
