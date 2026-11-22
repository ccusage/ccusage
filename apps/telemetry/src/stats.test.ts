import assert from 'node:assert/strict';
import { after, beforeEach, describe, it } from 'node:test';

const yesterday = new Date(Date.now() - 86_400_000).toISOString().slice(0, 10);
const cacheEntries = new Map<string, Response>();
const cacheDescriptor = Object.getOwnPropertyDescriptor(globalThis, 'caches');
Object.defineProperty(globalThis, 'caches', {
	configurable: true,
	value: {
		open: async (name: string) => {
			assert.equal(name, 'ccusage-public-stats');
			return {
				match: async (key: RequestInfo) =>
					cacheEntries.get(key instanceof Request ? key.url : String(key))?.clone(),
				put: async (key: RequestInfo, response: Response) => {
					cacheEntries.set(key instanceof Request ? key.url : String(key), response.clone());
				},
			};
		},
	},
});
const { app } = await import('./index.ts');
beforeEach(() => cacheEntries.clear());
after(() => {
	if (cacheDescriptor) Object.defineProperty(globalThis, 'caches', cacheDescriptor);
	else Reflect.deleteProperty(globalThis, 'caches');
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

void describe('public statistics', () => {
	void it('uses one KV snapshot read per cache miss and shares cached data across query strings', async () => {
		let reads = 0;
		const env = {
			ROLLUPS: {
				get: async (key: string) => {
					reads++;
					assert.equal(key, 'public-stats.json');
					return JSON.stringify(snapshot());
				},
			},
		};
		const first = await app.request('/v1/stats?first=1', {}, env);
		assert.equal(first.status, 200);
		assert.match(first.headers.get('cache-control') ?? '', /max-age=300, s-maxage=3600/);
		assert.equal(reads, 1);

		const body = await first.json();
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

		assert.equal((await app.request('/v1/stats?second=2', {}, env)).status, 200);
		assert.equal(reads, 1);
		assert.equal(cacheEntries.size, 1);
	});

	void it('returns a cacheable empty state before the first rollup without inventing zero days', async () => {
		const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => null } });
		assert.equal(response.status, 200);
		assert.match(response.headers.get('cache-control') ?? '', /max-age=300, s-maxage=3600/);
		assert.deepEqual(await response.json(), { generatedAt: null, days: [] });
	});

	void it('returns an uncacheable error when the snapshot is corrupt', async () => {
		const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => '{' } });
		assert.equal(response.status, 503);
		assert.equal(response.headers.get('cache-control'), 'no-store');
	});

	void it('returns an uncacheable error when the snapshot read fails', async () => {
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

	void it('rejects a snapshot containing more than 90 days', async () => {
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

void describe('dashboard delivery', () => {
	void it('serves the Vite-built dashboard asset with restrictive same-origin resource headers', async () => {
		const html = '<!doctype html><script type="module" src="/assets/dashboard.js"></script>';
		let servedRequest: RequestInfo | URL | undefined;
		const response = await app.request(
			'/',
			{},
			{
				ASSETS: {
					fetch: async (request: RequestInfo | URL) => {
						servedRequest = request;
						return new Response(html, {
							headers: { 'Content-Type': 'text/html; charset=utf-8' },
						});
					},
				},
			},
		);
		assert.equal(servedRequest instanceof Request ? new URL(servedRequest.url).pathname : '', '/');
		assert.equal(response.status, 200);
		assert.match(response.headers.get('content-security-policy') ?? '', /script-src 'self'/);
		assert.match(response.headers.get('content-security-policy') ?? '', /font-src 'self'/);
		assert.equal(response.headers.get('x-content-type-options'), 'nosniff');
		assert.equal(response.headers.get('referrer-policy'), 'no-referrer');
		assert.match(response.headers.get('cache-control') ?? '', /max-age=300/);
		assert.equal(await response.text(), html);
	});
});
