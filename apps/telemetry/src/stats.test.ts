import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import { Script } from 'node:vm';

import { app } from './index.ts';

void describe('public statistics', () => {
	void it('shares one cached aggregate response across query strings without reading R2 again', async () => {
		const descriptor = Object.getOwnPropertyDescriptor(globalThis, 'caches');
		const entries = new Map<string, Response>();
		Object.defineProperty(globalThis, 'caches', {
			configurable: true,
			value: {
				default: {
					match: async (request: Request) => entries.get(request.url)?.clone(),
					put: async (request: Request, response: Response) => {
						entries.set(request.url, response);
					},
				},
			},
		});
		try {
			let reads = 0;
			const env = {
				ROLLUPS: {
					get: async () => {
						reads++;
						return null;
					},
				},
			};
			assert.equal((await app.request('/v1/stats?first=1', {}, env)).status, 200);
			assert.equal(reads, 90);
			assert.equal((await app.request('/v1/stats?second=2', {}, env)).status, 200);
			assert.equal(reads, 90);
			assert.equal(entries.size, 1);
		} finally {
			if (descriptor) Object.defineProperty(globalThis, 'caches', descriptor);
			else Reflect.deleteProperty(globalThis, 'caches');
		}
	});
	void it('publishes aggregates without exposing IDs or arbitrary stored fields', async () => {
		const yesterday = new Date(Date.now() - 86_400_000).toISOString().slice(0, 10);
		const row = {
			date: yesterday,
			generatedAt: `${yesterday}T23:59:00Z`,
			dailyActiveUsers: 3,
			weeklyActiveUsers: 8,
			monthlyActiveUsers: 12,
			anonymousId: 'private',
			byPlatform: [{ os: 'linux', arch: 'x86_64', users: 3, anonymousId: 'private' }],
			byVersion: [{ version: '20.1.0', users: 3 }],
			byCommand: [{ command: 'daily', users: 3 }],
		};
		const response = await app.request(
			'/v1/stats',
			{},
			{
				ROLLUPS: {
					get: async (key: string) =>
						key === `daily/${yesterday}.json` ? { json: async () => row } : null,
				},
			},
		);
		assert.equal(response.status, 200);
		assert.match(response.headers.get('cache-control') ?? '', /s-maxage=3600/);
		const body = await response.text();
		assert.doesNotMatch(body, /anonymousId|private|byCommand/);
		const data = JSON.parse(body);
		assert.equal(data.days.length, 1);
		assert.equal(data.days[0].dailyActiveUsers, 3);
	});

	void it('keeps missing days absent instead of reporting zero activity', async () => {
		const response = await app.request('/v1/stats', {}, { ROLLUPS: { get: async () => null } });
		assert.deepEqual(await response.json(), { generatedAt: null, days: [] });
	});

	void it('returns an uncacheable error when the aggregate store fails', async () => {
		const response = await app.request(
			'/v1/stats',
			{},
			{
				ROLLUPS: {
					get: async () => {
						throw new Error('store failed');
					},
				},
			},
		);
		assert.equal(response.status, 503);
		assert.equal(response.headers.get('cache-control'), 'no-store');
	});

	void it('rejects corrupt aggregates instead of publishing misleading counts', async () => {
		const response = await app.request(
			'/v1/stats',
			{},
			{ ROLLUPS: { get: async () => ({ json: async () => ({ dailyActiveUsers: -1 }) }) } },
		);
		assert.equal(response.status, 503);
	});
});

void describe('dashboard delivery', () => {
	void it('serves a same-origin dashboard with an accessible data table and restrictive script policy', async () => {
		const response = await app.request('/');
		assert.equal(response.status, 200);
		assert.match(response.headers.get('content-security-policy') ?? '', /script-src 'self'/);
		const html = await response.text();
		assert.match(html, /<script src="\/dashboard.js" defer>/);
		assert.match(html, /<table>/);
		assert.match(html, /role="status"/);
		const script = await app.request('/dashboard.js');
		assert.equal(script.status, 200);
		assert.match(script.headers.get('content-type') ?? '', /text\/javascript/);
		new Script(await script.text());
	});
});
