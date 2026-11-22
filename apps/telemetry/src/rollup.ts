import * as z from 'zod';

const DATASET = 'events.analyticsEngine.ccusage_telemetry';
const DAY_MS = 24 * 60 * 60 * 1000;
export const PUBLIC_STATS_KEY = 'public-stats.json';
const MAX_PUBLIC_DAYS = 90;
const MAX_BREAKDOWN_ROWS = 20;

const count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const usersSchema = z.coerce.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const platformSchema = z.object({ os: z.string(), arch: z.string(), users: count });
const versionSchema = z.object({ version: z.string(), users: count });
const platformRowSchema = z.object({ os: z.string(), arch: z.string(), users: usersSchema });
const versionRowSchema = z.object({ version: z.string(), users: usersSchema });
const commandRowSchema = z.object({ command: z.string(), users: usersSchema });
const countRowSchema = z.object({ users: usersSchema, maxSampleInterval: usersSchema });
const samplingRowSchema = z.object({ maxSampleInterval: usersSchema });
const publicDaySchema = z.object({
	date: z.iso.date(),
	generatedAt: z.iso.datetime({ offset: true }),
	dailyActiveUsers: count,
	weeklyActiveUsers: count,
	monthlyActiveUsers: count,
	byPlatform: z.array(platformSchema),
	byVersion: z.array(versionSchema),
	samplingObserved: z.boolean().optional(),
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
	samplingObserved: z.boolean().optional(),
});

export type PublicDailyRollup = z.infer<typeof publicDaySchema>;
export type PublicStats = z.infer<typeof publicStatsSchema>;

export type RollupBindings = {
	ANALYTICS: { query(request: AnalyticsSQLQuery): Promise<{ data: unknown[] }> };
	ROLLUPS: {
		get(keys: string[]): Promise<Map<string, string | null>>;
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
	const [rollup] = await writeRollups(env, [date], now);
	if (!rollup) throw new Error('The daily rollup was not created');
	return rollup;
}

export async function writeRecentRollups(
	env: RollupBindings,
	latestDate: string,
	now: Date,
): Promise<DailyRollup[]> {
	if (!z.iso.date().safeParse(latestDate).success) {
		throw new Error('The rollup date must be a completed UTC day');
	}
	const latestDateMs = Date.parse(`${latestDate}T00:00:00Z`);
	const dates = [2, 1, 0].map((daysAgo) => isoDate(latestDateMs - daysAgo * DAY_MS));
	return writeRollups(env, dates, now);
}

async function writeRollups(
	env: RollupBindings,
	dates: string[],
	now: Date,
): Promise<DailyRollup[]> {
	const today = now.toISOString().slice(0, 10);
	if (dates.some((date) => !z.iso.date().safeParse(date).success || date >= today)) {
		throw new Error('The rollup date must be a completed UTC day');
	}

	const rollups = await Promise.all(
		dates.map((date) => buildDailyRollup(env.ANALYTICS, date, now)),
	);
	await Promise.all(
		rollups.map((rollup) =>
			env.ROLLUPS.put(`daily/${rollup.date}.json`, `${JSON.stringify(rollup, null, '\t')}\n`),
		),
	);
	await publishPublicStats(env.ROLLUPS, rollups, today);
	return rollups;
}

async function publishPublicStats(
	rollups: RollupBindings['ROLLUPS'],
	updatedRollups: DailyRollup[],
	today: string,
): Promise<void> {
	const cutoff = isoDate(Date.parse(`${today}T00:00:00Z`) - MAX_PUBLIC_DAYS * DAY_MS);
	const dates = Array.from({ length: MAX_PUBLIC_DAYS }, (_, index) =>
		isoDate(Date.parse(`${cutoff}T00:00:00Z`) + index * DAY_MS),
	);
	const keys = dates.map((date) => `daily/${date}.json`);
	const storedRollups = await rollups.get(keys);
	const updatedByDate = new Map(updatedRollups.map((rollup) => [rollup.date, rollup]));
	const days: PublicDailyRollup[] = [];
	for (const date of dates) {
		const key = `daily/${date}.json`;
		const updatedRollup = updatedByDate.get(date);
		const storedRollup = storedRollups.get(key);
		if (!updatedRollup && storedRollup == null) continue;
		const rollup = updatedRollup ?? dailyRollupSchema.parse(JSON.parse(storedRollup ?? ''));
		if (rollup.date !== date) throw new Error('Daily rollup archive date does not match its key');
		days.push(toPublicDailyRollup(rollup));
	}
	const latestDay = days.at(-1);
	if (latestDay) {
		const snapshot = publicStatsSchema.parse({ generatedAt: latestDay.generatedAt, days });
		await rollups.put(PUBLIC_STATS_KEY, JSON.stringify(snapshot));
	}
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
		...(rollup.samplingObserved === undefined ? {} : { samplingObserved: rollup.samplingObserved }),
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
		dailyActiveUsers: daily.users,
		weeklyActiveUsers: weekly.users,
		monthlyActiveUsers: monthly.users,
		byPlatform: byPlatform.rows,
		byVersion: byVersion.rows,
		byCommand: byCommand.rows,
		samplingObserved: [
			daily.samplingObserved,
			weekly.samplingObserved,
			monthly.samplingObserved,
			byPlatform.samplingObserved,
			byVersion.samplingObserved,
			byCommand.samplingObserved,
		].some(Boolean),
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
): Promise<{ users: number; samplingObserved: boolean }> {
	const result = await analytics.query({
		query: `SELECT count(DISTINCT index1) AS users, approx_max(sampleInterval) AS maxSampleInterval FROM ${DATASET} WHERE timestamp >= $start AND timestamp < $end`,
		params: { start, end },
	});
	const rows = z.array(countRowSchema).length(1).parse(result.data);
	const row = rows[0];
	if (!row) throw new Error('Analytics Engine returned no count row');
	return { users: row.users, samplingObserved: row.maxSampleInterval > 1 };
}

async function breakdown<Row>(
	analytics: RollupBindings['ANALYTICS'],
	{ start, end }: AnalyticsWindow,
	columns: string,
	groupBy: string,
	rowSchema: z.ZodType<Row>,
): Promise<{ rows: Row[]; samplingObserved: boolean }> {
	const result = await analytics.query({
		query: `SELECT ${columns}, count(DISTINCT index1) AS users, approx_max(sampleInterval) AS maxSampleInterval FROM ${DATASET} WHERE timestamp >= $start AND timestamp < $end GROUP BY ${groupBy} ORDER BY users DESC LIMIT ${MAX_BREAKDOWN_ROWS}`,
		params: { start, end },
	});
	const samplingRows = z.array(samplingRowSchema).parse(result.data);
	const rows = z.array(rowSchema).parse(result.data);
	return { rows, samplingObserved: samplingRows.some((row) => row.maxSampleInterval > 1) };
}

function isoDate(ms: number): string {
	return new Date(ms).toISOString().slice(0, 10);
}
