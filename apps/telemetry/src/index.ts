import { Hono } from 'hono';

import type { Env } from './env.ts';
import { previousUtcDay, writeRecentRollups } from './rollup.ts';
import { purgePublicStatsCache, stats } from './stats.ts';
import { v1 } from './v1/index.ts';

export const app = new Hono<Env>()
	.get('/', async (c) => {
		const asset = await c.env.ASSETS.fetch(c.req.raw);
		const headers = new Headers(asset.headers);
		headers.set(
			'Content-Security-Policy',
			"default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'",
		);
		headers.set('Cache-Control', 'public, max-age=300');
		headers.set('X-Content-Type-Options', 'nosniff');
		headers.set('Referrer-Policy', 'no-referrer');
		return new Response(asset.body, {
			status: asset.status,
			statusText: asset.statusText,
			headers,
		});
	})
	.route('/v1', stats)
	.route('/v1', v1);

export default {
	fetch: app.fetch,
	scheduled(controller, env, ctx) {
		ctx.waitUntil(
			writeRecentRollups(env, previousUtcDay(controller.scheduledTime), new Date()).then(() =>
				purgePublicStatsCache(ctx.cache),
			),
		);
	},
} satisfies ExportedHandler<Env['Bindings']>;
