import { cloudflare } from '@cloudflare/vite-plugin';
import { defineConfig } from 'vite';

export default defineConfig({
	build: { assetsInlineLimit: 0, minify: true },
	plugins: [cloudflare()],
});
