import type { Context } from 'hono';
import { Hono } from 'hono';
import type { CloudEnv } from 'void';
import { defineHandler } from 'void';

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

export async function readPublicStats(
	rollups: Pick<KVNamespace, 'get'>,
	now: number,
): Promise<PublicStats | null> {
	const stored = await rollups.get(PUBLIC_STATS_KEY);
	return stored === null
		? null
		: parsePublicStats(stored, new Date(now).toISOString().slice(0, 10));
}

export function cachePublicStats(c: Context, snapshot: PublicStats | null, now: number): void {
	const edgeSeconds = edgeMaxAge(snapshot, now);
	c.header('Cache-Control', `public, max-age=${Math.min(BROWSER_MAX_AGE_SECONDS, edgeSeconds)}`);
	c.header('Cloudflare-CDN-Cache-Control', `public, max-age=${edgeSeconds}`);
	c.header('Cache-Tag', PUBLIC_STATS_CACHE_TAG);
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

export const getPublicStats = defineHandler(async (c) => {
	const now = Date.now();
	let snapshot: PublicStats | null;
	try {
		snapshot = await readPublicStats(c.env.ROLLUPS, now);
	} catch {
		c.header('Cache-Control', 'no-store');
		return c.json({ error: 'Statistics are temporarily unavailable.' }, 503);
	}
	cachePublicStats(c, snapshot, now);
	c.header('X-Content-Type-Options', 'nosniff');
	return c.json(snapshot ?? { generatedAt: null, days: [] });
});

if (import.meta.vitest) {
	const app = new Hono<CloudEnv>().get('/v1/stats', async (c) => {
		const response = await getPublicStats(c);
		assert.instanceOf(response, Response);
		return response;
	});

	const stored = {
		generatedAt: '2026-11-25T00:15:42Z',
		days: [
			{
				date: '2026-11-24',
				generatedAt: '2026-11-25T00:15:42Z',
				dailyActiveUsers: 3,
				weeklyActiveUsers: 8,
				monthlyActiveUsers: 12,
				byPlatform: [{ os: 'linux', arch: 'x86_64', users: 3, private: 'discard' }],
				byVersion: [{ version: '20.1.0', users: 3 }],
				byCommand: [{ command: 'daily', users: 3 }],
				anonymousId: 'private',
			},
		],
		private: 'discard',
	};

	describe('GET /v1/stats', () => {
		beforeEach(() => {
			vi.useFakeTimers({ now: Date.parse('2026-11-25T12:00:00Z'), toFake: ['Date'] });
		});
		afterEach(() => {
			vi.useRealTimers();
		});

		it('is the handler the route file exports as GET', async () => {
			const route = await import('../routes/v1/stats.ts');

			expect(route.GET).toBe(getPublicStats);
		});

		it('publishes only public snapshot fields', async () => {
			const get = vi.fn(async () => JSON.stringify(stored));

			const response = await app.request('/v1/stats', {}, { ROLLUPS: { get } });

			expect(get.mock.calls).toEqual([[PUBLIC_STATS_KEY]]);
			expect(response.status).toBe(200);
			expect(response.headers.get('x-content-type-options')).toBe('nosniff');
			expect(await response.json()).toEqual({
				generatedAt: '2026-11-25T00:15:42Z',
				days: [
					{
						date: '2026-11-24',
						generatedAt: '2026-11-25T00:15:42Z',
						dailyActiveUsers: 3,
						weeklyActiveUsers: 8,
						monthlyActiveUsers: 12,
						byPlatform: [{ os: 'linux', arch: 'x86_64', users: 3 }],
						byVersion: [{ version: '20.1.0', users: 3 }],
					},
				],
			});
		});

		it('caches a current snapshot at the edge until the next rollup under the purge tag', async () => {
			const response = await app.request(
				'/v1/stats',
				{},
				{ ROLLUPS: { get: async () => JSON.stringify(stored) } },
			);

			expect(response.headers.get('cloudflare-cdn-cache-control')).toBe('public, max-age=44100');
			expect(response.headers.get('cache-control')).toBe('public, max-age=300');
			expect(response.headers.get('cache-tag')).toBe('public-stats');
		});

		it('keeps a snapshot without yesterday at the edge only briefly', async () => {
			vi.setSystemTime(Date.parse('2026-11-25T00:15:10Z'));
			const pending = {
				...stored,
				generatedAt: '2026-11-24T00:15:42Z',
				days: [{ ...stored.days[0], date: '2026-11-23' }],
			};

			const response = await app.request(
				'/v1/stats',
				{},
				{ ROLLUPS: { get: async () => JSON.stringify(pending) } },
			);

			expect(response.status).toBe(200);
			expect(response.headers.get('cloudflare-cdn-cache-control')).toBe('public, max-age=60');
			expect(response.headers.get('cache-control')).toBe('public, max-age=60');
		});

		it('returns a briefly cached empty state before the first rollup without inventing zero days', async () => {
			const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => null } });

			expect(response.status).toBe(200);
			expect(response.headers.get('cloudflare-cdn-cache-control')).toBe('public, max-age=60');
			expect(await response.json()).toEqual({ generatedAt: null, days: [] });
		});

		it('returns an uncacheable error when the snapshot is corrupt', async () => {
			const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => '{' } });

			expect(response.status).toBe(503);
			expect(response.headers.get('cache-control')).toBe('no-store');
		});

		it('returns an uncacheable error when the snapshot read fails', async () => {
			const response = await app.request(
				'/v1/stats',
				{},
				{ ROLLUPS: { get: vi.fn().mockRejectedValue(new Error('KV unavailable')) } },
			);

			expect(response.status).toBe(503);
			expect(response.headers.get('cache-control')).toBe('no-store');
		});

		it('rejects a snapshot containing more than 90 days', async () => {
			const days = Array.from({ length: 91 }, (_, index) => ({
				date: new Date(Date.UTC(2026, 10, 24 - 90 + index)).toISOString().slice(0, 10),
				generatedAt: '2026-11-25T00:15:42Z',
				dailyActiveUsers: 1,
				weeklyActiveUsers: 1,
				monthlyActiveUsers: 1,
				byPlatform: [],
				byVersion: [],
			}));

			const response = await app.request(
				'/v1/stats',
				{},
				{
					ROLLUPS: {
						get: async () => JSON.stringify({ generatedAt: '2026-11-25T00:15:42Z', days }),
					},
				},
			);

			expect(response.status).toBe(503);
		});
	});

	describe('purgePublicStatsCache', () => {
		it('purges the public statistics tag', async () => {
			const purge = vi.fn(async () => ({ success: true, errors: [] }));

			await purgePublicStatsCache({
				purge,
				invalidate: async () => ({ success: true, errors: [] }),
			});

			expect(purge.mock.calls).toEqual([[{ tags: ['public-stats'] }]]);
		});

		it('fails when Workers Cache is not enabled', async () => {
			await expect(purgePublicStatsCache(undefined)).rejects.toThrow(
				'Workers Cache is not enabled',
			);
		});

		it('surfaces purge failures', async () => {
			await expect(
				purgePublicStatsCache({
					purge: async () => ({ success: false, errors: [{ code: 1, message: 'denied' }] }),
					invalidate: async () => ({ success: true, errors: [] }),
				}),
			).rejects.toThrow('denied');
		});
	});
}
