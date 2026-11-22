import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import {
	previousUtcDay,
	type RollupBindings,
	writeDailyRollup,
	writeRecentRollups,
} from './rollup.ts';

type Put = { key: string; value: string };

function priorDay(index: number, latest = '2026-10-20') {
	return shiftDay(latest, -index);
}

function shiftDay(date: string, offset: number) {
	return new Date(Date.parse(`${date}T00:00:00Z`) + offset * 86_400_000).toISOString().slice(0, 10);
}

function archive(date: string) {
	return JSON.stringify({
		date,
		generatedAt: `${date}T23:59:00Z`,
		dailyActiveUsers: 2,
		weeklyActiveUsers: 4,
		monthlyActiveUsers: 7,
		byPlatform: [{ os: 'linux', arch: 'x86_64', users: 2 }],
		byVersion: [{ version: '20.0.37', users: 2 }],
		byCommand: [{ command: 'daily', users: 2 }],
	});
}

function archiveHistory(start: string, end: string, gaps: string[] = []) {
	const values: Record<string, string> = {};
	for (let date = start; date <= end; date = shiftDay(date, 1)) {
		if (!gaps.includes(date)) values[`daily/${date}.json`] = archive(date);
	}
	return values;
}

function fakeEnv(initial: Record<string, string> = {}) {
	const queries: AnalyticsSQLQuery[] = [];
	const gets: string[][] = [];
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
						{ os: 'macos', arch: 'aarch64', users: '7', maxSampleInterval: '1' },
						{ os: 'linux', arch: 'x86_64', users: '3', maxSampleInterval: '1' },
					];
				}
				if (query.includes('GROUP BY version')) {
					return [{ version: '20.0.38', users: '10', maxSampleInterval: '1' }];
				}
				if (query.includes('GROUP BY command')) {
					return [
						{ command: 'daily', users: '6', maxSampleInterval: '1' },
						{ command: 'statusline', users: '4', maxSampleInterval: '1' },
					];
				}
				if (start === '2026-09-22T00:00:00.000Z') {
					return [{ users: '42', maxSampleInterval: '1' }];
				}
				if (start === '2026-10-15T00:00:00.000Z') {
					return [{ users: '25', maxSampleInterval: '1' }];
				}
				return [{ users: '10', maxSampleInterval: '1' }];
			})();
			return { data: rows };
		},
	};
	const rollups = {
		get: async (keys: string[]) => {
			gets.push(keys);
			return new Map(keys.map((key) => [key, values.get(key) ?? null]));
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
	void it('archives the daily rollup and publishes a sanitized 90-day snapshot from archives', async () => {
		const initial = {
			...archiveHistory('2026-07-23', '2026-10-20'),
			'public-stats.json': '{',
		};
		const { env, gets, puts, values } = fakeEnv(initial);

		const rollup = await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.deepEqual(rollup, {
			date: '2026-10-21',
			generatedAt: '2026-10-22T00:15:00.000Z',
			dailyActiveUsers: 10,
			weeklyActiveUsers: 25,
			monthlyActiveUsers: 42,
			samplingObserved: false,
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
		assert.equal(gets.length, 1);
		assert.equal(gets[0]?.length, 90);
		assert.equal(gets[0]?.includes('public-stats.json'), false);
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
		assert.equal('samplingObserved' in publicSnapshot.days[0], false);
		assert.deepEqual(publicSnapshot.days.at(-1), {
			date: '2026-10-21',
			generatedAt: '2026-10-22T00:15:00.000Z',
			dailyActiveUsers: 10,
			weeklyActiveUsers: 25,
			monthlyActiveUsers: 42,
			samplingObserved: false,
			byPlatform: [
				{ os: 'macos', arch: 'aarch64', users: 7 },
				{ os: 'linux', arch: 'x86_64', users: 3 },
			],
			byVersion: [{ version: '20.0.38', users: 10 }],
		});
	});

	void it('publishes a first snapshot without history backfill', async () => {
		const { env, gets, puts, values } = fakeEnv();

		await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.equal(gets[0]?.length, 90);
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

	void it('replaces an older day without dropping newer archive data', async () => {
		const { env, puts, values } = fakeEnv(archiveHistory('2026-07-24', '2026-10-21'));

		await writeDailyRollup(env, '2026-10-20', new Date('2026-10-22T00:15:00Z'));

		const snapshot = JSON.parse(values.get('public-stats.json') ?? '');
		assert.equal(snapshot.days.length, 90);
		assert.equal(snapshot.days.at(-1).date, '2026-10-21');
		assert.equal(snapshot.generatedAt, '2026-10-21T23:59:00Z');
		const replaced = snapshot.days.find((day: { date: string }) => day.date === '2026-10-20');
		assert.ok(replaced);
		assert.equal(replaced.dailyActiveUsers, 10);
		assert.equal(replaced.samplingObserved, false);
		assert.equal(puts.length, 2);
	});

	void it('fails closed when an authoritative archive is corrupt', async () => {
		const { env, puts, values } = fakeEnv({
			...archiveHistory('2026-07-24', '2026-10-20'),
			'daily/2026-10-01.json': '{',
			'public-stats.json': '{',
		});

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.deepEqual(
			puts.map(({ key }) => key),
			['daily/2026-10-21.json'],
		);
		assert.equal(values.get('public-stats.json'), '{');
	});

	void it('bounds every query to whole UTC days ending at midnight after the date', async () => {
		const { env, queries } = fakeEnv();

		await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.equal(queries.length, 6);
		for (const { query } of queries) {
			assert.match(query, /FROM events\.analyticsEngine\.ccusage_telemetry WHERE/);
			assert.match(query, /timestamp >= \$start AND timestamp < \$end/);
			assert.match(query, /count\(DISTINCT index1\) AS users/);
			assert.match(query, /approx_max\(sampleInterval\) AS maxSampleInterval/);
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

	void it('marks observed sampling without scaling distinct-user counts', async () => {
		const { env } = fakeEnv();
		const query = env.ANALYTICS.query.bind(env.ANALYTICS);
		env.ANALYTICS.query = async (request) => {
			if (request.query.includes('GROUP BY version')) {
				return { data: [{ version: '20.0.38', users: '10', maxSampleInterval: '3' }] };
			}
			const result = await query(request);
			return {
				data: result.data,
			};
		};

		const rollup = await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.equal(rollup.samplingObserved, true);
		assert.equal(rollup.dailyActiveUsers, 10);
		assert.deepEqual(rollup.byVersion, [{ version: '20.0.38', users: 10 }]);
	});

	void it('limits all breakdown queries while preserving full distinct-user totals', async () => {
		const { env, queries } = fakeEnv();

		const rollup = await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		const breakdownQueries = queries.filter(({ query }) => query.includes('GROUP BY'));
		assert.equal(breakdownQueries.length, 3);
		for (const { query } of breakdownQueries) {
			assert.match(query, /ORDER BY users DESC LIMIT 20$/);
		}
		const totalsQueries = queries.filter(({ query }) => !query.includes('GROUP BY'));
		assert.equal(totalsQueries.length, 3);
		for (const { query } of totalsQueries) assert.equal(query.includes('LIMIT'), false);
		assert.deepEqual(
			[rollup.dailyActiveUsers, rollup.weeklyActiveUsers, rollup.monthlyActiveUsers],
			[10, 25, 42],
		);
	});

	void it('does not publish zeros when Analytics Engine returns no count row', async () => {
		const { env, puts } = fakeEnv();
		env.ANALYTICS.query = async () => ({ data: [] });

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.equal(puts.length, 0);
	});

	void it('rejects rows that do not match the expected shape instead of writing them', async () => {
		const { env, puts } = fakeEnv();
		env.ANALYTICS.query = async () => ({ data: [{ users: 'many' }] });

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.equal(puts.length, 0);
	});
});

void describe('writeRecentRollups', () => {
	void it('recovers recent missed dates and rebuilds a corrupt public snapshot within 23 operations', async () => {
		const { env, queries, gets, puts, values } = fakeEnv({
			...archiveHistory('2026-07-24', '2026-10-18'),
			'public-stats.json': '{',
		});

		const rollups = await writeRecentRollups(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.deepEqual(
			rollups.map(({ date }) => date),
			['2026-10-19', '2026-10-20', '2026-10-21'],
		);
		assert.equal(queries.length, 18);
		assert.equal(gets.length, 1);
		assert.equal(gets[0]?.length, 90);
		assert.deepEqual(
			puts.map(({ key }) => key),
			[
				'daily/2026-10-19.json',
				'daily/2026-10-20.json',
				'daily/2026-10-21.json',
				'public-stats.json',
			],
		);
		assert.equal(queries.length + gets.length + puts.length, 23);
		const snapshot = JSON.parse(values.get('public-stats.json') ?? '');
		assert.equal(snapshot.days.length, 90);
		assert.deepEqual(
			snapshot.days.slice(-3).map((day: { date: string }) => day.date),
			['2026-10-19', '2026-10-20', '2026-10-21'],
		);
		assert.equal(snapshot.generatedAt, '2026-10-22T00:15:00.000Z');
	});

	void it('keeps genuine archive gaps instead of creating zero-valued days', async () => {
		const { env, values } = fakeEnv({
			...archiveHistory('2026-07-24', '2026-10-18', ['2026-10-05']),
		});

		await writeRecentRollups(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		const snapshot = JSON.parse(values.get('public-stats.json') ?? '');
		assert.equal(snapshot.days.length, 89);
		assert.equal(
			snapshot.days.some((day: { date: string }) => day.date === '2026-10-05'),
			false,
		);
		assert.equal(
			snapshot.days.every((day: { dailyActiveUsers: number }) => day.dailyActiveUsers > 0),
			true,
		);
	});

	void it('fails closed if an authoritative archive cannot be parsed', async () => {
		const { env, puts, values } = fakeEnv({
			...archiveHistory('2026-07-24', '2026-10-18'),
			'daily/2026-10-05.json': '{',
			'public-stats.json': '{',
		});

		await assert.rejects(writeRecentRollups(env, '2026-10-21', new Date('2026-10-22T00:15:00Z')));
		assert.deepEqual(
			puts.map(({ key }) => key),
			['daily/2026-10-19.json', 'daily/2026-10-20.json', 'daily/2026-10-21.json'],
		);
		assert.equal(values.get('public-stats.json'), '{');
	});
});
