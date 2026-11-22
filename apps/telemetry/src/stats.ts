import { Hono } from 'hono';
import { cache } from 'hono/cache';

import type { Env } from './env.ts';
import { parsePublicStats, PUBLIC_STATS_KEY } from './rollup.ts';

export const stats = new Hono<Env>().get(
	'/stats',
	cache({
		cacheName: 'ccusage-public-stats',
		wait: true,
		cacheControl: 'public, max-age=300, s-maxage=3600',
		keyGenerator: (c) => new URL('/v1/stats', c.req.url).toString(),
		cacheableStatusCodes: [200],
		onCacheNotAvailable: false,
	}),
	async (c) => {
		try {
			const stored = await c.env.ROLLUPS.get(PUBLIC_STATS_KEY);
			const response =
				stored === null
					? c.json({ generatedAt: null, days: [] })
					: c.json(parsePublicStats(stored));
			response.headers.set('X-Content-Type-Options', 'nosniff');
			return response;
		} catch {
			c.header('Cache-Control', 'no-store');
			return c.json({ error: 'Statistics are temporarily unavailable.' }, 503);
		}
	},
);
