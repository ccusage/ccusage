import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { previousUtcDay, type RollupBindings, writeDailyRollup } from './rollup.ts';

type Put = { key: string; value: string };

function priorDay(index: number, latest = '2026-10-20') {
	return new Date(Date.parse(`${latest}T00:00:00Z`) - index * 86_400_000)
		.toISOString()
		.slice(0, 10);
}

function priorSnapshot(latest = '2026-10-20') {
	const days = Array.from({ length: 90 }, (_, index) => ({
		date: priorDay(89 - index, latest),
		generatedAt: `${priorDay(89 - index, latest)}T23:59:00Z`,
		dailyActiveUsers: 2,
		weeklyActiveUsers: 4,
		monthlyActiveUsers: 7,
		byPlatform: [{ os: 'linux', arch: 'x86_64', users: 2, private: 'discard' }],
		byVersion: [{ version: '20.0.37', users: 2 }],
		byCommand: [{ command: 'daily', users: 2 }],
		anonymousId: 'private',
	}));
	return { generatedAt: `${latest}T23:59:00Z`, days, private: 'discard' };
}

function fakeEnv(initial: Record<string, string> = {}) {
	const queries: AnalyticsSQLQuery[] = [];
	const gets: string[] = [];
	const puts: Put[] = [];
	const values = new Map(Object.entries(initial));
	const analytics = {
		query: async (request: AnalyticsSQLQuery) => {
			const { query } = request;
			const start = Object.entries(request.params ?? {}).find(([key]) => key === 'start')?.[1];
			queries.push(request);
			const rows = (() => {
				if (query.includes('GROUP BY os, arch')) {
					return [
						{ os: 'macos', arch: 'aarch64', users: '7' },
						{ os: 'linux', arch: 'x86_64', users: '3' },
					];
				}
				if (query.includes('GROUP BY version')) {
					return [{ version: '20.0.38', users: '10' }];
				}
				if (query.includes('GROUP BY command')) {
					return [
						{ command: 'daily', users: '6' },
						{ command: 'statusline', users: '4' },
					];
				}
				if (start === '2026-09-22T00:00:00.000Z') {
					return [{ users: '42' }];
				}
				if (start === '2026-10-15T00:00:00.000Z') {
					return [{ users: '25' }];
				}
				return [{ users: '10' }];
			})();
			return { data: rows };
		},
	};
	const rollups = {
		get: async (key: string) => {
			gets.push(key);
			return values.get(key) ?? null;
		},
		put: async (key: string, value: string) => {
			puts.push({ key, value });
			values.set(key, value);
		},
	};
	const env: RollupBindings = { ANALYTICS: analytics, ROLLUPS: rollups };
	return { env, queries, gets, puts, values };
}

void describe('previousUtcDay', () => {
	void it('returns the UTC day before the trigger fired', () => {
		assert.equal(previousUtcDay(Date.UTC(2026, 9, 22, 0, 15)), '2026-10-21');
		assert.equal(previousUtcDay(Date.UTC(2026, 0, 1, 0, 15)), '2025-12-31');
	});
});

void describe('writeDailyRollup', () => {
	void it('archives the daily rollup and publishes a sanitized 90-day snapshot', async () => {
		const { env, gets, puts, values } = fakeEnv({
			'public-stats.json': JSON.stringify(priorSnapshot()),
		});

		const rollup = await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.deepEqual(rollup, {
			date: '2026-10-21',
			generatedAt: '2026-10-22T00:15:00.000Z',
			dailyActiveUsers: 10,
			weeklyActiveUsers: 25,
			monthlyActiveUsers: 42,
			byPlatform: [
				{ os: 'macos', arch: 'aarch64', users: 7 },
				{ os: 'linux', arch: 'x86_64', users: 3 },
			],
			byVersion: [{ version: '20.0.38', users: 10 }],
			byCommand: [
				{ command: 'daily', users: 6 },
				{ command: 'statusline', users: 4 },
			],
		});
		assert.deepEqual(gets, ['public-stats.json']);
		assert.deepEqual(
			puts.map(({ key }) => key),
			['daily/2026-10-21.json', 'public-stats.json'],
		);
		assert.deepEqual(JSON.parse(values.get('daily/2026-10-21.json') ?? ''), rollup);

		const publicSnapshot = JSON.parse(values.get('public-stats.json') ?? '');
		assert.equal(publicSnapshot.generatedAt, '2026-10-22T00:15:00.000Z');
		assert.equal(publicSnapshot.days.length, 90);
		assert.equal(publicSnapshot.days[0].date, priorDay(88, '2026-10-20'));
		assert.equal(publicSnapshot.days.at(-1).date, '2026-10-21');
		assert.equal(JSON.stringify(publicSnapshot).includes('anonymousId'), false);
		assert.equal(JSON.stringify(publicSnapshot).includes('private'), false);
		assert.equal(JSON.stringify(publicSnapshot).includes('byCommand'), false);
		assert.deepEqual(publicSnapshot.days.at(-1), {
			date: '2026-10-21',
			generatedAt: '2026-10-22T00:15:00.000Z',
			dailyActiveUsers: 10,
			weeklyActiveUsers: 25,
			monthlyActiveUsers: 42,
			byPlatform: [
				{ os: 'macos', arch: 'aarch64', users: 7 },
				{ os: 'linux', arch: 'x86_64', users: 3 },
			],
			byVersion: [{ version: '20.0.38', users: 10 }],
		});
	});

	void it('creates the first public snapshot with one KV read and no history backfill', async () => {
		const { env, gets, puts, values } = fakeEnv();

		await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.deepEqual(gets, ['public-stats.json']);
		assert.deepEqual(
			puts.map(({ key }) => key),
			['daily/2026-10-21.json', 'public-stats.json'],
		);
		const snapshot = JSON.parse(values.get('public-stats.json') ?? '');
		assert.deepEqual(
			snapshot.days.map((day: { date: string }) => day.date),
			['2026-10-21'],
		);
	});

	void it('replaces an older day without dropping newer public data', async () => {
		const stored = priorSnapshot('2026-10-21');
		const { env, puts, values } = fakeEnv({ 'public-stats.json': JSON.stringify(stored) });

		await writeDailyRollup(env, '2026-10-20', new Date('2026-10-22T00:15:00Z'));

		const snapshot = JSON.parse(values.get('public-stats.json') ?? '');
		assert.equal(snapshot.days.length, 90);
		assert.equal(snapshot.days.at(-1).date, '2026-10-21');
		assert.equal(snapshot.generatedAt, '2026-10-21T23:59:00Z');
		const replaced = snapshot.days.find((day: { date: string }) => day.date === '2026-10-20');
		assert.ok(replaced);
		assert.equal(replaced.dailyActiveUsers, 10);
		assert.equal(puts.length, 2);
	});

	void it('does not overwrite stored data when the existing snapshot is corrupt', async () => {
		const { env, puts } = fakeEnv({ 'public-stats.json': '{' });

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.equal(puts.length, 0);
	});

	void it('bounds every query to whole UTC days ending at midnight after the date', async () => {
		const { env, queries } = fakeEnv({ 'public-stats.json': JSON.stringify(priorSnapshot()) });

		await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.equal(queries.length, 6);
		for (const { query } of queries) {
			assert.match(query, /FROM events\.analyticsEngine\.ccusage_telemetry WHERE/);
			assert.match(query, /timestamp >= \$start AND timestamp < \$end/);
			assert.match(query, /count\(DISTINCT index1\) AS users/);
			assert.equal(query.includes('toDateTime'), false);
		}
		assert.deepEqual(
			new Set(queries.map(({ params }) => JSON.stringify(params))),
			new Set([
				JSON.stringify({ start: '2026-10-21T00:00:00.000Z', end: '2026-10-22T00:00:00.000Z' }),
				JSON.stringify({ start: '2026-10-15T00:00:00.000Z', end: '2026-10-22T00:00:00.000Z' }),
				JSON.stringify({ start: '2026-09-22T00:00:00.000Z', end: '2026-10-22T00:00:00.000Z' }),
			]),
		);
	});

	void it('does not publish zeros when Analytics Engine returns no count row', async () => {
		const { env, puts } = fakeEnv({ 'public-stats.json': JSON.stringify(priorSnapshot()) });
		env.ANALYTICS.query = async ({ query }) => ({ data: query.includes('GROUP BY') ? [] : [] });

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.equal(puts.length, 0);
	});

	void it('rejects rows that do not match the expected shape instead of writing them', async () => {
		const { env, puts } = fakeEnv({ 'public-stats.json': JSON.stringify(priorSnapshot()) });
		env.ANALYTICS.query = async () => ({ data: [{ users: 'many' }] });

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.equal(puts.length, 0);
	});
});
