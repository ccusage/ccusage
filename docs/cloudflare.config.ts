import { defineConfig } from 'cf/config';

export default defineConfig({
	accountId: 'e721638321cf56dac84472ad22a161ed',
	worker: {
		name: 'ccusage-guide',
		compatibilityDate: '2025-07-21',
		previewUrls: true,
		assets: {
			notFoundHandling: '404-page',
		},
	},
});
