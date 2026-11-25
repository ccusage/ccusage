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
