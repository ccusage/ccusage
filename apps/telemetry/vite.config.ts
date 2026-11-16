import { cloudflare } from '@cloudflare/vite-plugin';
import { defineConfig } from 'vite';

export default defineConfig({
	// Vite leaves SSR output unminified by default, which roughly doubles the
	// Worker bundle.
	build: { minify: true },
	plugins: [cloudflare()],
});
