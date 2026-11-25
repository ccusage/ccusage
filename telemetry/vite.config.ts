import { voidSvelte } from '@void/svelte/plugin';
import { voidPlugin } from 'void';
import { defineConfig } from 'vitest/config';

export default defineConfig({
	build: { assetsInlineLimit: 0, minify: true },
	define: { 'import.meta.vitest': 'undefined' },
	plugins: [voidPlugin(), voidSvelte()],
	test: {
		globals: true,
		include: ['src/**/*.test.ts'],
		includeSource: ['{src,crons}/**/*.ts'],
		env: { TZ: 'UTC' },
	},
});
