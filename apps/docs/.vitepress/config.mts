import { defineConfig } from 'vitepress';

const REPO = 'https://github.com/usehuxley/huxley';

/** Links to files outside the site (or to internal documents) go to GitHub. */
function repoLink(href: string): string | null {
	if (/^[a-z]+:|^#|^\//i.test(href)) return null;
	const [path, hash = ''] = href.split('#');
	const internal = /^(\.\/)?(adr\/|DESIGN_DOCUMENT|IMPLEMENTATION_PLAN|api-conventions|performance)/.test(path);
	// The schema is published with the site.
	if (/^(\.\.\/)+schemas\//.test(path)) return path.replace(/^(\.\.\/)+/, '/');
	if (path.startsWith('../')) {
		return `${REPO}/blob/main/${path.replace(/^(\.\.\/)+/, '')}${hash ? `#${hash}` : ''}`;
	}
	if (internal) {
		return `${REPO}/blob/main/docs/${path.replace(/^\.\//, '')}${hash ? `#${hash}` : ''}`;
	}
	return null;
}

export default defineConfig({
	title: 'Huxley',
	description: 'Documentation for Huxley, the automation platform for MSPs.',
	// Pages are copied from the repository's docs/ (scripts/sync-pages.mjs).
	srcDir: 'src',
	cleanUrls: true,
	lastUpdated: false,
	head: [['meta', { name: 'theme-color', content: '#4f46e5' }]],
	markdown: {
		config(md) {
			// The docs are full of {{ expressions }}: in text and inline code
			// they're shown, not read as Vue (code blocks already are).
			const shown = (html: string) => html.replaceAll('{{', '&#123;&#123;').replaceAll('}}', '&#125;&#125;');
			for (const rule of ['text', 'code_inline'] as const) {
				const original = md.renderer.rules[rule]!;
				md.renderer.rules[rule] = (tokens, idx, options, env, self) =>
					shown(original(tokens, idx, options, env, self));
			}
			const render =
				md.renderer.rules.link_open ?? ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
			md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
				const href = tokens[idx].attrGet('href');
				const outside = href ? repoLink(href) : null;
				if (outside) tokens[idx].attrSet('href', outside);
				return render(tokens, idx, options, env, self);
			};
		}
	},
	themeConfig: {
		nav: [
			{ text: 'Guide', link: '/getting-started' },
			{ text: 'Workflows', link: '/workflows' },
			{ text: 'HWF schema', link: '/schemas/hwf/v1.json', target: '_blank' }
		],
		sidebar: [
			{
				text: 'Getting started',
				items: [
					{ text: 'Introduction', link: '/' },
					{ text: 'Your first workflow', link: '/getting-started' },
					{ text: 'Search', link: '/search' }
				]
			},
			{
				text: 'Automation',
				items: [
					{ text: 'Workflows', link: '/workflows' },
					{ text: 'Expressions', link: '/expressions' },
					{ text: 'TypeScript SDK', link: '/sdk' },
					{ text: 'Integrations', link: '/integrations' },
					{ text: 'Forms', link: '/forms' },
					{ text: 'Scripts and runners', link: '/runners' },
					{ text: 'Datasets', link: '/datasets' }
				]
			},
			{
				text: 'AI',
				items: [
					{ text: 'AI providers and steps', link: '/ai' },
					{ text: 'The assistant', link: '/assistant' },
					{ text: 'MCP server', link: '/mcp' }
				]
			},
			{
				text: 'Modules',
				items: [
					{ text: 'Clients and contacts', link: '/crm' },
					{ text: 'Service desk', link: '/servicedesk' },
					{ text: 'Workforce', link: '/workforce' },
					{ text: 'Assets', link: '/assets' },
					{ text: 'Knowledge base', link: '/knowledge' },
					{ text: 'Credential vault', link: '/vault' },
					{ text: 'Product catalog', link: '/catalog' },
					{ text: 'Agreements', link: '/agreements' },
					{ text: 'Projects', link: '/projects' },
					{ text: 'Sales and quoting', link: '/sales' },
					{ text: 'Inventory', link: '/inventory' },
					{ text: 'Purchasing', link: '/purchasing' },
					{ text: 'Invoicing', link: '/billing' },
					{ text: 'Client portal', link: '/portal' },
					{ text: 'Satisfaction and NPS', link: '/csat' },
					{ text: 'Client health', link: '/health' },
					{ text: 'Changes and problems', link: '/changes' },
					{ text: 'Reporting data', link: '/analytics' },
					{ text: 'ConnectWise Automate', link: '/cwautomate' },
					{ text: 'ImmyBot', link: '/immybot' },
					{ text: 'QuickBooks Online', link: '/quickbooks' }
				]
			},
			{
				text: 'Sharing',
				items: [{ text: 'Packs', link: '/packs' }]
			}
		],
		search: { provider: 'local' },
		socialLinks: [{ icon: 'github', link: REPO }],
		editLink: { pattern: `${REPO}/edit/main/docs/:path`, text: 'Edit this page' },
		footer: { message: 'Released under the MIT License.' }
	}
});
