import assert from 'node:assert/strict';
import { Hono } from 'hono';
import type { CloudEnv } from 'void';

import { loadDashboard, type Props } from '../pages/index.server.ts';
import { GET as stats } from '../routes/v1/stats.ts';
import { purgePublicStatsCache } from './stats.ts';

const yesterday = new Date(Date.now() - 86_400_000).toISOString().slice(0, 10);
let loaded: Props | undefined;
const app = new Hono<CloudEnv>()
	.get('/v1/stats', async (c) => {
		const response = await stats(c);
		if (!(response instanceof Response)) throw new Error('The stats route must return a Response');
		return response;
	})
	.get('/', async (c) => {
		loaded = await loadDashboard(c);
		return c.body(null);
	});

function snapshot(extra: Record<string, unknown> = {}) {
	return {
		generatedAt: `${yesterday}T23:59:00Z`,
		days: [
			{
				date: yesterday,
				generatedAt: `${yesterday}T23:59:00Z`,
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
		...extra,
	};
}

describe('public statistics', () => {
	it('publishes only public snapshot fields with Workers Cache headers', async () => {
		const response = await app.request(
			'/v1/stats',
			{},
			{
				ROLLUPS: {
					get: async (key: string) => {
						assert.equal(key, 'public-stats.json');
						return JSON.stringify(snapshot());
					},
				},
			},
		);
		assert.equal(response.status, 200);
		assert.equal(response.headers.get('cache-tag'), 'public-stats');
		assert.match(response.headers.get('cache-control') ?? '', /^public, max-age=\d+$/);
		assert.match(
			response.headers.get('cloudflare-cdn-cache-control') ?? '',
			/^public, max-age=\d+$/,
		);

		const body = await response.json();
		assert.equal(JSON.stringify(body).includes('private'), false);
		assert.equal(JSON.stringify(body).includes('byCommand'), false);
		assert.deepEqual(body, {
			generatedAt: `${yesterday}T23:59:00Z`,
			days: [
				{
					date: yesterday,
					generatedAt: `${yesterday}T23:59:00Z`,
					dailyActiveUsers: 3,
					weeklyActiveUsers: 8,
					monthlyActiveUsers: 12,
					byPlatform: [{ os: 'linux', arch: 'x86_64', users: 3 }],
					byVersion: [{ version: '20.1.0', users: 3 }],
				},
			],
		});
	});

	it('caches a current snapshot at the edge until the next rollup', async () => {
		onTestFinished(() => {
			vi.useRealTimers();
		});
		vi.useFakeTimers({ now: Date.parse('2026-11-25T12:00:00Z'), toFake: ['Date'] });
		const current = snapshot({
			generatedAt: '2026-11-25T00:15:42Z',
			days: [{ ...snapshot().days[0], date: '2026-11-24' }],
		});
		const response = await app.request(
			'/v1/stats',
			{},
			{ ROLLUPS: { get: async () => JSON.stringify(current) } },
		);
		assert.equal(response.status, 200);
		assert.equal(response.headers.get('cloudflare-cdn-cache-control'), 'public, max-age=44100');
		assert.equal(response.headers.get('cache-control'), 'public, max-age=300');
	});

	it('keeps a snapshot without yesterday at the edge only briefly', async () => {
		onTestFinished(() => {
			vi.useRealTimers();
		});
		vi.useFakeTimers({ now: Date.parse('2026-11-25T00:15:10Z'), toFake: ['Date'] });
		const pending = snapshot({
			generatedAt: '2026-11-24T00:15:42Z',
			days: [{ ...snapshot().days[0], date: '2026-11-23' }],
		});
		const response = await app.request(
			'/v1/stats',
			{},
			{ ROLLUPS: { get: async () => JSON.stringify(pending) } },
		);
		assert.equal(response.status, 200);
		assert.equal(response.headers.get('cloudflare-cdn-cache-control'), 'public, max-age=60');
		assert.equal(response.headers.get('cache-control'), 'public, max-age=60');
	});

	it('purges the public statistics tag and surfaces purge failures', async () => {
		const purged: unknown[] = [];
		await purgePublicStatsCache({
			purge: async (options) => {
				purged.push(options);
				return { success: true, errors: [] };
			},
			invalidate: async () => ({ success: true, errors: [] }),
		});
		assert.deepEqual(purged, [{ tags: ['public-stats'] }]);

		await assert.rejects(purgePublicStatsCache(undefined), /Workers Cache is not enabled/);
		await assert.rejects(
			purgePublicStatsCache({
				purge: async () => ({ success: false, errors: [{ code: 1, message: 'denied' }] }),
				invalidate: async () => ({ success: true, errors: [] }),
			}),
			/denied/,
		);
	});

	it('returns a briefly cached empty state before the first rollup without inventing zero days', async () => {
		const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => null } });
		assert.equal(response.status, 200);
		assert.equal(response.headers.get('cloudflare-cdn-cache-control'), 'public, max-age=60');
		assert.deepEqual(await response.json(), { generatedAt: null, days: [] });
	});

	it('returns an uncacheable error when the snapshot is corrupt', async () => {
		const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => '{' } });
		assert.equal(response.status, 503);
		assert.equal(response.headers.get('cache-control'), 'no-store');
	});

	it('returns an uncacheable error when the snapshot read fails', async () => {
		const response = await app.request(
			'/v1/stats',
			{},
			{
				ROLLUPS: {
					get: async () => {
						throw new Error('KV unavailable');
					},
				},
			},
		);
		assert.equal(response.status, 503);
		assert.equal(response.headers.get('cache-control'), 'no-store');
	});

	it('rejects a snapshot containing more than 90 days', async () => {
		const day = {
			date: yesterday,
			generatedAt: `${yesterday}T23:59:00Z`,
			dailyActiveUsers: 1,
			weeklyActiveUsers: 1,
			monthlyActiveUsers: 1,
			byPlatform: [],
			byVersion: [],
		};
		const days = Array.from({ length: 91 }, (_, index) => ({
			...day,
			date: new Date(Date.parse(`${yesterday}T00:00:00Z`) - index * 86_400_000)
				.toISOString()
				.slice(0, 10),
		}));
		const response = await app.request(
			'/v1/stats',
			{},
			{
				ROLLUPS: {
					get: async () => JSON.stringify({ generatedAt: day.generatedAt, days }),
				},
			},
		);
		assert.equal(response.status, 503);
	});
});

describe('dashboard page', () => {
	it('loads the snapshot with restrictive headers and the same edge lifetime as the API', async () => {
		onTestFinished(() => {
			vi.useRealTimers();
		});
		vi.useFakeTimers({ now: Date.parse('2026-11-25T12:00:00Z'), toFake: ['Date'] });
		const current = snapshot({
			generatedAt: '2026-11-25T00:15:42Z',
			days: [{ ...snapshot().days[0], date: '2026-11-24' }],
		});
		const response = await app.request(
			'/',
			{},
			{ ROLLUPS: { get: async () => JSON.stringify(current) } },
		);
		assert.equal(response.status, 200);
		assert.match(response.headers.get('content-security-policy') ?? '', /script-src 'self'/);
		assert.equal(response.headers.get('x-content-type-options'), 'nosniff');
		assert.equal(response.headers.get('referrer-policy'), 'no-referrer');
		assert.equal(response.headers.get('cloudflare-cdn-cache-control'), 'public, max-age=44100');
		assert.equal(response.headers.get('cache-tag'), 'public-stats');
		assert.equal(loaded?.unavailable, false);
		assert.equal(loaded.stats?.days[0]?.date, '2026-11-24');
		assert.equal(JSON.stringify(loaded).includes('byCommand'), false);
	});

	it('renders an uncacheable unavailable state when the snapshot cannot be read', async () => {
		const response = await app.request('/', {}, { ROLLUPS: { get: async () => '{' } });
		assert.equal(response.status, 200);
		assert.equal(response.headers.get('cache-control'), 'no-store');
		assert.equal(loaded?.stats, null);
		assert.equal(loaded.unavailable, true);
	});
});
