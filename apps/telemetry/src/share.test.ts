import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { OTHER_KEY, shareView } from './dashboard/share.ts';
import type { PublicDailyRollup } from './rollup.ts';

function day(
	date: string,
	dailyActiveUsers: number,
	versions: [string, number][],
): PublicDailyRollup {
	return {
		date,
		generatedAt: `${date}T23:59:00Z`,
		dailyActiveUsers,
		weeklyActiveUsers: dailyActiveUsers,
		monthlyActiveUsers: dailyActiveUsers,
		byPlatform: [],
		byVersion: versions.map(([version, users]) => ({ version, users })),
	};
}

void describe('share view', () => {
	void it('keeps the five largest groups of the latest day and folds the rest into Other', () => {
		const days = [
			day('2026-11-23', 10, [
				['1', 6],
				['2', 4],
			]),
			day('2026-11-24', 100, [
				['a', 40],
				['b', 20],
				['c', 10],
				['d', 9],
				['e', 8],
				['f', 7],
			]),
		];
		const view = shareView(days, 'version', ['2026-11-22', '2026-11-23', '2026-11-24']);

		assert.deepEqual(view.keys, ['a', 'b', 'c', 'd', 'e', OTHER_KEY]);
		assert.deepEqual(
			view.rows.filter((row) => row.date === '2026-11-24').map(({ key, users }) => [key, users]),
			[
				['a', 40],
				['b', 20],
				['c', 10],
				['d', 9],
				['e', 8],
				[OTHER_KEY, 13],
			],
		);
		assert.deepEqual(
			view.rows.filter((row) => row.date === '2026-11-23').map(({ key, users }) => [key, users]),
			[
				['a', 0],
				['b', 0],
				['c', 0],
				['d', 0],
				['e', 0],
				[OTHER_KEY, 10],
			],
		);
		assert.equal(
			view.rows.some((row) => row.date === '2026-11-22'),
			false,
		);
		assert.equal(view.latest?.groups.length, 6);
	});

	void it('never reports a negative remainder when sampled groups exceed the daily count', () => {
		const view = shareView(
			[
				day('2026-11-24', 5, [
					['a', 4],
					['b', 3],
				]),
			],
			'version',
			['2026-11-24'],
		);
		assert.deepEqual(
			view.rows.map(({ key, users }) => [key, users]),
			[
				['a', 4],
				['b', 3],
				[OTHER_KEY, 0],
			],
		);
	});

	void it('has no series before the first aggregate', () => {
		assert.deepEqual(shareView([], 'platform', []), {
			keys: [],
			latest: null,
			rows: [],
			dates: [],
		});
	});
});
