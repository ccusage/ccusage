import type { PublicDailyRollup } from '../rollup.ts';

export const SHARE_SERIES_LIMIT = 5;
export const OTHER_KEY = 'Other';

export type ShareGroup = { label: string; users: number };
export type ShareDimension = 'platform' | 'version';
export type ShareRow = { date: string; key: string; users: number; share: number };
export type ShareView = {
	keys: readonly string[];
	latest: { date: string; total: number; groups: readonly ShareGroup[] } | null;
	rows: readonly ShareRow[];
	dates: readonly string[];
};

export function shareGroups(day: PublicDailyRollup, dimension: ShareDimension): ShareGroup[] {
	const groups =
		dimension === 'platform'
			? day.byPlatform.map(({ os, arch, users }) => ({ label: `${os} / ${arch}`, users }))
			: day.byVersion.map(({ version, users }) => ({ label: version, users }));
	return groups.toSorted((left, right) => right.users - left.users);
}

// NOTE(2026-11-25): Each installation sends at most one ping per UTC day with one platform and one version, so a day's groups are disjoint and the daily active count minus the listed groups is the unlisted remainder. Sampling can make that difference negative, which is clamped to zero.
export function shareView(
	days: readonly PublicDailyRollup[],
	dimension: ShareDimension,
	dates: readonly string[],
): ShareView {
	const latestDay = days.at(-1);
	if (!latestDay) return { keys: [], latest: null, rows: [], dates };
	const named = shareGroups(latestDay, dimension)
		.slice(0, SHARE_SERIES_LIMIT)
		.map(({ label }) => label);
	const keys = [...named, OTHER_KEY];
	const byDate = new Map(days.map((day) => [day.date, day]));
	const rows = dates.flatMap((date) => {
		const day = byDate.get(date);
		if (!day) return [];
		const groups = new Map(shareGroups(day, dimension).map(({ label, users }) => [label, users]));
		const total = day.dailyActiveUsers;
		const namedUsers = named.map((key) => groups.get(key) ?? 0);
		const otherUsers = Math.max(0, total - namedUsers.reduce((sum, users) => sum + users, 0));
		return [...namedUsers, otherUsers].map((users, index) => ({
			date,
			key: keys[index] ?? OTHER_KEY,
			users,
			share: total === 0 ? 0 : users / total,
		}));
	});
	return {
		keys,
		latest: {
			date: latestDay.date,
			total: latestDay.dailyActiveUsers,
			groups: shareGroups(latestDay, dimension),
		},
		rows,
		dates,
	};
}

export function seriesColorVariable(keys: readonly string[], key: string): string {
	const index = keys.indexOf(key);
	return key === OTHER_KEY || index < 0 ? '--series-other' : `--series-${index + 1}`;
}

if (import.meta.vitest) {
	describe('shareView', () => {
		it('keeps the five largest groups of the latest day and folds the rest into Other', () => {
			const days = [
				{
					date: '2026-11-23',
					generatedAt: '2026-11-23T23:59:00Z',
					dailyActiveUsers: 10,
					weeklyActiveUsers: 10,
					monthlyActiveUsers: 10,
					byPlatform: [],
					byVersion: [
						{ version: '1', users: 6 },
						{ version: '2', users: 4 },
					],
				},
				{
					date: '2026-11-24',
					generatedAt: '2026-11-24T23:59:00Z',
					dailyActiveUsers: 100,
					weeklyActiveUsers: 100,
					monthlyActiveUsers: 100,
					byPlatform: [],
					byVersion: [
						{ version: 'a', users: 40 },
						{ version: 'b', users: 20 },
						{ version: 'c', users: 10 },
						{ version: 'd', users: 9 },
						{ version: 'e', users: 8 },
						{ version: 'f', users: 7 },
					],
				},
			] satisfies PublicDailyRollup[];

			const view = shareView(days, 'version', ['2026-11-22', '2026-11-23', '2026-11-24']);

			expect(view.keys).toEqual(['a', 'b', 'c', 'd', 'e', OTHER_KEY]);
			expect(view.rows.map(({ date, key, users }) => [date, key, users])).toEqual([
				['2026-11-23', 'a', 0],
				['2026-11-23', 'b', 0],
				['2026-11-23', 'c', 0],
				['2026-11-23', 'd', 0],
				['2026-11-23', 'e', 0],
				['2026-11-23', OTHER_KEY, 10],
				['2026-11-24', 'a', 40],
				['2026-11-24', 'b', 20],
				['2026-11-24', 'c', 10],
				['2026-11-24', 'd', 9],
				['2026-11-24', 'e', 8],
				['2026-11-24', OTHER_KEY, 13],
			]);
			expect(view.latest?.groups).toHaveLength(6);
		});

		it('never reports a negative remainder when sampled groups exceed the daily count', () => {
			const days = [
				{
					date: '2026-11-24',
					generatedAt: '2026-11-24T23:59:00Z',
					dailyActiveUsers: 5,
					weeklyActiveUsers: 5,
					monthlyActiveUsers: 5,
					byPlatform: [],
					byVersion: [
						{ version: 'a', users: 4 },
						{ version: 'b', users: 3 },
					],
				},
			] satisfies PublicDailyRollup[];

			const view = shareView(days, 'version', ['2026-11-24']);

			expect(view.rows.map(({ key, users }) => [key, users])).toEqual([
				['a', 4],
				['b', 3],
				[OTHER_KEY, 0],
			]);
		});

		it('has no series before the first aggregate', () => {
			expect(shareView([], 'platform', [])).toEqual({
				keys: [],
				latest: null,
				rows: [],
				dates: [],
			});
		});
	});
}
