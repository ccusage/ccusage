import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { previousUtcDay, type RollupBindings, writeDailyRollup } from './rollup.ts';

type Put = { key: string; body: string; contentType: string | undefined };

/**
 * Answers each rollup query from canned rows, matched by what the query groups
 * by and how wide its time window is.
 */
function fakeEnv() {
	const queries: string[] = [];
	const puts: Put[] = [];
	const analytics = {
		query: async ({ query }: AnalyticsSQLQuery) => {
			queries.push(query);
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
				if (query.includes("toDateTime('2026-09-22 00:00:00')")) {
					return [{ users: '42' }];
				}
				if (query.includes("toDateTime('2026-10-15 00:00:00')")) {
					return [{ users: '25' }];
				}
				return [{ users: '10' }];
			})();
			return { data: rows };
		},
	};
	const rollups = {
		put: async (key: string, body: string, options?: R2PutOptions) => {
			const metadata = options?.httpMetadata;
			puts.push({
				key,
				body,
				contentType: metadata instanceof Headers ? undefined : metadata?.contentType,
			});
			return null;
		},
	};
	const env: RollupBindings = { ANALYTICS: analytics, ROLLUPS: rollups };
	return { env, queries, puts };
}

void describe('previousUtcDay', () => {
	void it('returns the UTC day before the trigger fired', () => {
		assert.equal(previousUtcDay(Date.UTC(2026, 9, 22, 0, 15)), '2026-10-21');
		assert.equal(previousUtcDay(Date.UTC(2026, 0, 1, 0, 15)), '2025-12-31');
	});
});

void describe('writeDailyRollup', () => {
	void it('writes the day, its rolling windows, and its breakdowns to R2', async () => {
		const { env, puts } = fakeEnv();

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
		assert.equal(puts.length, 1);
		assert.equal(puts[0]?.key, 'daily/2026-10-21.json');
		assert.equal(puts[0]?.contentType, 'application/json');
		assert.deepEqual(JSON.parse(puts[0]?.body ?? ''), rollup);
	});

	void it('bounds every query to whole UTC days ending at midnight after the date', async () => {
		const { env, queries } = fakeEnv();

		await writeDailyRollup(env, '2026-10-21', new Date('2026-10-22T00:15:00Z'));

		assert.equal(queries.length, 6);
		for (const query of queries) {
			assert.match(query, /FROM events\.analyticsEngine\.ccusage_telemetry WHERE/);
			assert.match(query, /timestamp < toDateTime\('2026-10-22 00:00:00'\)/);
			assert.match(query, /count\(DISTINCT index1\) AS users/);
		}
		const starts = queries.map((query) => /timestamp >= toDateTime\('([^']+)'\)/.exec(query)?.[1]);
		assert.deepEqual(
			new Set(starts),
			new Set(['2026-10-21 00:00:00', '2026-10-15 00:00:00', '2026-09-22 00:00:00']),
		);
	});

	void it('rejects rows that do not match the expected shape instead of writing them', async () => {
		const { env, puts } = fakeEnv();
		env.ANALYTICS.query = async () => ({ data: [{ users: 'many' }] });

		await assert.rejects(writeDailyRollup(env, '2026-10-21', new Date()));
		assert.equal(puts.length, 0);
	});
});
