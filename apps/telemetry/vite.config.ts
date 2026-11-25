import { voidSvelte } from '@void/svelte/plugin';
import { defineConfig } from 'vite';
import { voidPlugin } from 'void';

export default defineConfig({
	build: { assetsInlineLimit: 0, minify: true },
	plugins: [voidPlugin(), voidSvelte()],
});
