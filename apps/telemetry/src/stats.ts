import { Hono } from 'hono';

import type { Env } from './env.ts';
import type { PublicStats } from './rollup.ts';
import { nextRollupTime, parsePublicStats, previousUtcDay, PUBLIC_STATS_KEY } from './rollup.ts';

const PUBLIC_STATS_CACHE_TAG = 'public-stats';
const BROWSER_MAX_AGE_SECONDS = 300;
const PENDING_ROLLUP_MAX_AGE_SECONDS = 60;

// NOTE(2026-11-25): A KV read can still return the previous snapshot right after the rollup purges the cache tag. Caching a snapshot without yesterday only briefly keeps such a read from hiding the new aggregate until the next rollup.
function edgeMaxAge(snapshot: PublicStats | null, now: number): number {
	if (snapshot?.days.at(-1)?.date !== previousUtcDay(now)) return PENDING_ROLLUP_MAX_AGE_SECONDS;
	return Math.max(PENDING_ROLLUP_MAX_AGE_SECONDS, Math.floor((nextRollupTime(now) - now) / 1000));
}

export async function purgePublicStatsCache(cache: CacheContext | undefined): Promise<void> {
	if (!cache) throw new Error('Workers Cache is not enabled for this Worker');
	const result = await cache.purge({ tags: [PUBLIC_STATS_CACHE_TAG] });
	if (!result.success) {
		throw new Error(
			`Purging public statistics failed: ${result.errors.map((error) => error.message).join(', ')}`,
		);
	}
}

export const stats = new Hono<Env>().get('/stats', async (c) => {
	let snapshot: PublicStats | null;
	try {
		const stored = await c.env.ROLLUPS.get(PUBLIC_STATS_KEY);
		snapshot = stored === null ? null : parsePublicStats(stored);
	} catch {
		c.header('Cache-Control', 'no-store');
		return c.json({ error: 'Statistics are temporarily unavailable.' }, 503);
	}

	const edgeSeconds = edgeMaxAge(snapshot, Date.now());
	c.header('Cache-Control', `public, max-age=${Math.min(BROWSER_MAX_AGE_SECONDS, edgeSeconds)}`);
	c.header('Cloudflare-CDN-Cache-Control', `public, max-age=${edgeSeconds}`);
	c.header('Cache-Tag', PUBLIC_STATS_CACHE_TAG);
	c.header('X-Content-Type-Options', 'nosniff');
	return c.json(snapshot ?? { generatedAt: null, days: [] });
});
