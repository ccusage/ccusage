import { cloudflareRedirect } from '@ryoppippi/vite-plugin-cloudflare-redirect';
import { defineConfig } from 'vitepress';
import { groupIconMdPlugin, groupIconVitePlugin } from 'vitepress-plugin-group-icons';
import llmstxt from 'vitepress-plugin-llms';

export default defineConfig({
	title: 'ccusage',
	description: 'Usage analysis tool for coding (agent) CLIs',
	base: '/',
	cleanUrls: true,
	ignoreDeadLinks: true,

	head: [
		['link', { rel: 'icon', href: '/favicon.svg' }],
		['meta', { name: 'theme-color', content: '#646cff' }],
		['meta', { property: 'og:type', content: 'website' }],
		['meta', { property: 'og:locale', content: 'en' }],
		['meta', { property: 'og:title', content: 'ccusage | Coding (Agent) CLI Usage Analysis' }],
		['meta', { property: 'og:site_name', content: 'ccusage' }],
		[
			'meta',
			{
				property: 'og:image',
				content: 'https://cdn.jsdelivr.net/gh/ryoppippi/ccusage@main/docs/public/logo.png',
			},
		],
		['meta', { property: 'og:url', content: 'https://github.com/ryoppippi/ccusage' }],
	],

	themeConfig: {
		logo: '/logo.svg',

		nav: [
			{ text: 'Guide', link: '/guide/' },
			{
				text: 'Links',
				items: [
					{ text: 'GitHub', link: 'https://github.com/ryoppippi/ccusage' },
					{ text: 'npm', link: 'https://www.npmjs.com/package/ccusage' },
					{ text: 'Changelog', link: 'https://github.com/ryoppippi/ccusage/releases' },
					{ text: 'DeepWiki', link: 'https://deepwiki.com/ryoppippi/ccusage' },
					{ text: 'Package Stats', link: 'https://tanstack.com/ccusage?npmPackage=ccusage' },
					{ text: 'Sponsor', link: 'https://github.com/sponsors/ryoppippi' },
				],
			},
		],

		sidebar: {
			'/guide/': [
				{
					text: 'Introduction',
					items: [
						{ text: 'Introduction', link: '/guide/' },
						{ text: 'Getting Started', link: '/guide/getting-started' },
						{ text: 'Installation', link: '/guide/installation' },
					],
				},
				{
					text: 'Usage Views',
					items: [
						{ text: 'All Sources (Default)', link: '/guide/all-reports' },
						{ text: 'Daily Usage', link: '/guide/daily-reports' },
						{ text: 'Weekly Usage', link: '/guide/weekly-reports' },
						{ text: 'Monthly Usage', link: '/guide/monthly-reports' },
						{ text: 'Session Usage', link: '/guide/session-reports' },
					],
				},
				{
					text: 'Data Sources',
					items: [
						{ text: 'Claude Code', link: '/guide/claude/' },
						{ text: 'Codex', link: '/guide/codex/' },
						{ text: 'OpenCode', link: '/guide/opencode/' },
						{ text: 'Amp', link: '/guide/amp/' },
						{ text: 'Droid', link: '/guide/droid/' },
						{ text: 'Codebuff', link: '/guide/codebuff/' },
						{ text: 'Hermes Agent', link: '/guide/hermes/' },
						{ text: 'pi-agent', link: '/guide/pi/' },
						{ text: 'Goose', link: '/guide/goose/' },
						{ text: 'Kilo', link: '/guide/kilo/' },
						{ text: 'Qwen', link: '/guide/qwen/' },
						{ text: 'GitHub Copilot CLI', link: '/guide/copilot/' },
						{ text: 'Gemini CLI', link: '/guide/gemini/' },
						{ text: 'Kimi', link: '/guide/kimi/' },
						{ text: 'OpenClaw', link: '/guide/openclaw/' },
						{ text: 'Source Support Q&A', link: '/guide/source-support-qa' },
					],
				},
				{
					text: 'Configuration',
					items: [
						{ text: 'Overview', link: '/guide/configuration' },
						{ text: 'Command-Line Options', link: '/guide/cli-options' },
						{ text: 'Environment Variables', link: '/guide/environment-variables' },
						{ text: 'Configuration Files', link: '/guide/config-files' },
						{ text: 'Cost Calculation Modes', link: '/guide/cost-modes' },
					],
				},
				{
					text: 'Integration',
					items: [{ text: 'JSON Output', link: '/guide/json-output' }],
				},
				{
					text: 'Community',
					items: [
						{ text: 'Community Projects', link: '/guide/community-projects' },
						{ text: 'Sponsors', link: '/guide/sponsors' },
					],
				},
			],
		},

		socialLinks: [
			{ icon: 'github', link: 'https://github.com/ryoppippi/ccusage' },
			{ icon: 'npm', link: 'https://www.npmjs.com/package/ccusage' },
			{ icon: 'twitter', link: 'https://x.com/cc_usage' },
		],

		footer: {
			message: 'Released under the MIT License.',
			copyright: 'Copyright © 2025 ryoppippi',
		},

		search: {
			provider: 'local',
		},

		editLink: {
			pattern: 'https://github.com/ryoppippi/ccusage/edit/main/docs/:path',
			text: 'Edit this page on GitHub',
		},

		lastUpdated: {
			text: 'Updated at',
			formatOptions: {
				year: 'numeric',
				month: '2-digit',
				day: '2-digit',
				hour: '2-digit',
				minute: '2-digit',
				hour12: false,
				timeZone: 'UTC',
			},
		},
	},

	vite: {
		plugins: [
			cloudflareRedirect({
				mode: 'generate',
				entries: [
					{ from: '/raycast', to: 'https://www.raycast.com/nyatinte/ccusage', status: 302 },
					{ from: '/gh', to: 'https://github.com/ryoppippi/ccusage', status: 302 },
					{ from: '/npm', to: 'https://www.npmjs.com/package/ccusage', status: 302 },
					{ from: '/deepwiki', to: 'https://deepwiki.com/ryoppippi/ccusage', status: 302 },
					{ from: '/sponsor', to: 'https://github.com/sponsors/ryoppippi', status: 302 },
					{ from: '/guide/custom-paths', to: '/guide/claude/', status: 301 },
					{ from: '/guide/directory-detection', to: '/guide/claude/', status: 301 },
					{ from: '/guide/related-projects', to: '/guide/community-projects', status: 301 },
				],
			}) as any,
			groupIconVitePlugin(),
			...llmstxt(),
		],
	},

	markdown: {
		config(md) {
			// eslint-disable-next-line ts/no-unsafe-argument -- markdown-it type mismatch between vitepress and plugin
			md.use(groupIconMdPlugin as any);
		},
	},
});
