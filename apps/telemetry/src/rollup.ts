import * as z from 'zod';

const DATASET = 'events.analyticsEngine.ccusage_telemetry';
const DAY_MS = 24 * 60 * 60 * 1000;
export const PUBLIC_STATS_KEY = 'public-stats.json';
const MAX_PUBLIC_DAYS = 90;

const count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const usersSchema = z.coerce.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const platformSchema = z.object({ os: z.string(), arch: z.string(), users: count });
const versionSchema = z.object({ version: z.string(), users: count });
const platformRowSchema = z.object({ os: z.string(), arch: z.string(), users: usersSchema });
const versionRowSchema = z.object({ version: z.string(), users: usersSchema });
const commandRowSchema = z.object({ command: z.string(), users: usersSchema });
const countRowSchema = z.object({ users: usersSchema });
const publicDaySchema = z.object({
	date: z.iso.date(),
	generatedAt: z.iso.datetime({ offset: true }),
	dailyActiveUsers: count,
	weeklyActiveUsers: count,
	monthlyActiveUsers: count,
	byPlatform: z.array(platformSchema),
	byVersion: z.array(versionSchema),
});
const publicStatsSchema = z.object({
	generatedAt: z.iso.datetime({ offset: true }),
	days: z.array(publicDaySchema).min(1).max(MAX_PUBLIC_DAYS),
});
const dailyRollupSchema = z.object({
	date: z.iso.date(),
	generatedAt: z.iso.datetime({ offset: true }),
	dailyActiveUsers: count,
	weeklyActiveUsers: count,
	monthlyActiveUsers: count,
	byPlatform: z.array(platformSchema),
	byVersion: z.array(versionSchema),
	byCommand: z.array(z.object({ command: z.string(), users: count })),
});

export type PublicDailyRollup = z.infer<typeof publicDaySchema>;
export type PublicStats = z.infer<typeof publicStatsSchema>;

export type RollupBindings = {
	ANALYTICS: { query(request: AnalyticsSQLQuery): Promise<{ data: unknown[] }> };
	ROLLUPS: {
		get(key: string): Promise<string | null>;
		put(key: string, value: string): Promise<unknown>;
	};
};

export type DailyRollup = z.infer<typeof dailyRollupSchema>;

type AnalyticsWindow = { start: string; end: string };

export function parsePublicStats(stored: string, today = new Date().toISOString().slice(0, 10)) {
	const snapshot = publicStatsSchema.parse(JSON.parse(stored));
	let previousDate = '';
	for (const day of snapshot.days) {
		if (day.date >= today || day.date <= previousDate) {
			throw new Error('Public statistics dates are invalid');
		}
		previousDate = day.date;
	}
	return snapshot;
}

export function previousUtcDay(scheduledTime: number): string {
	return isoDate(scheduledTime - DAY_MS);
}

export async function writeDailyRollup(
	env: RollupBindings,
	date: string,
	now: Date,
): Promise<DailyRollup> {
	const today = now.toISOString().slice(0, 10);
	if (!z.iso.date().safeParse(date).success || date >= today) {
		throw new Error('The rollup date must be a completed UTC day');
	}

	const rollup = await buildDailyRollup(env.ANALYTICS, date, now);
	const storedSnapshot = await env.ROLLUPS.get(PUBLIC_STATS_KEY);
	const previousDays = storedSnapshot === null ? [] : parsePublicStats(storedSnapshot, today).days;
	const cutoff = isoDate(Date.parse(`${today}T00:00:00Z`) - MAX_PUBLIC_DAYS * DAY_MS);
	const daysByDate = new Map(
		previousDays.filter((day) => day.date >= cutoff).map((day) => [day.date, day]),
	);
	if (date >= cutoff) daysByDate.set(date, toPublicDailyRollup(rollup));
	const days = Array.from(daysByDate.values())
		.sort((left, right) => left.date.localeCompare(right.date))
		.slice(-MAX_PUBLIC_DAYS);
	const latestDay = days.at(-1);
	await env.ROLLUPS.put(`daily/${date}.json`, `${JSON.stringify(rollup, null, '\t')}\n`);
	if (latestDay) {
		const snapshot = publicStatsSchema.parse({ generatedAt: latestDay.generatedAt, days });
		await env.ROLLUPS.put(PUBLIC_STATS_KEY, JSON.stringify(snapshot));
	}
	return rollup;
}

function toPublicDailyRollup(rollup: DailyRollup): PublicDailyRollup {
	return {
		date: rollup.date,
		generatedAt: rollup.generatedAt,
		dailyActiveUsers: rollup.dailyActiveUsers,
		weeklyActiveUsers: rollup.weeklyActiveUsers,
		monthlyActiveUsers: rollup.monthlyActiveUsers,
		byPlatform: rollup.byPlatform,
		byVersion: rollup.byVersion,
	};
}

async function buildDailyRollup(
	analytics: RollupBindings['ANALYTICS'],
	date: string,
	now: Date,
): Promise<DailyRollup> {
	const dayEnd = Date.parse(`${date}T00:00:00Z`) + DAY_MS;
	const day = window(dayEnd, 1);
	const [daily, weekly, monthly, byPlatform, byVersion, byCommand] = await Promise.all([
		distinctUsers(analytics, day),
		distinctUsers(analytics, window(dayEnd, 7)),
		distinctUsers(analytics, window(dayEnd, 30)),
		breakdown(analytics, day, 'blob3 AS os, blob4 AS arch', 'os, arch', platformRowSchema),
		breakdown(analytics, day, 'blob2 AS version', 'version', versionRowSchema),
		breakdown(analytics, day, 'blob5 AS command', 'command', commandRowSchema),
	]);
	return {
		date,
		generatedAt: now.toISOString(),
		dailyActiveUsers: daily,
		weeklyActiveUsers: weekly,
		monthlyActiveUsers: monthly,
		byPlatform,
		byVersion,
		byCommand,
	};
}

function window(endMs: number, days: number): AnalyticsWindow {
	return {
		start: new Date(endMs - days * DAY_MS).toISOString(),
		end: new Date(endMs).toISOString(),
	};
}

async function distinctUsers(
	analytics: RollupBindings['ANALYTICS'],
	{ start, end }: AnalyticsWindow,
): Promise<number> {
	const result = await analytics.query({
		query: `SELECT count(DISTINCT index1) AS users FROM ${DATASET} WHERE timestamp >= $start AND timestamp < $end`,
		params: { start, end },
	});
	const rows = z.array(countRowSchema).length(1).parse(result.data);
	const row = rows[0];
	if (!row) throw new Error('Analytics Engine returned no count row');
	return row.users;
}

async function breakdown<Row>(
	analytics: RollupBindings['ANALYTICS'],
	{ start, end }: AnalyticsWindow,
	columns: string,
	groupBy: string,
	rowSchema: z.ZodType<Row>,
): Promise<Row[]> {
	const result = await analytics.query({
		query: `SELECT ${columns}, count(DISTINCT index1) AS users FROM ${DATASET} WHERE timestamp >= $start AND timestamp < $end GROUP BY ${groupBy} ORDER BY users DESC`,
		params: { start, end },
	});
	return z.array(rowSchema).parse(result.data);
}

function isoDate(ms: number): string {
	return new Date(ms).toISOString().slice(0, 10);
}
