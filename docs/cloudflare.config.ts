import { defineConfig } from 'cf/config';

export default defineConfig({
	worker: {
		name: 'ccusage-guide',
		compatibilityDate: '2025-07-21',
		previewUrls: true,
		assets: {
			notFoundHandling: '404-page',
		},
	},
});
