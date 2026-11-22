import * as z from 'zod';

/** Analytics Engine table that `PINGS` writes to; see `cloudflare.config.ts`. */
const DATASET = 'events.analyticsEngine.ccusage_telemetry';
const DAY_MS = 24 * 60 * 60 * 1000;

// Analytics Engine returns 64-bit counts as JSON strings, so they are coerced.
const usersSchema = z.coerce.number().int().nonnegative();
const countRowSchema = z.object({ users: usersSchema });
const platformRowSchema = z.object({ os: z.string(), arch: z.string(), users: usersSchema });
const versionRowSchema = z.object({ version: z.string(), users: usersSchema });
const commandRowSchema = z.object({ command: z.string(), users: usersSchema });

/**
 * The part of the Worker bindings the rollup uses. Rows are typed `unknown`
 * because they are validated with zod rather than trusted.
 */
export type RollupBindings = {
	ANALYTICS: { query(request: AnalyticsSQLQuery): Promise<{ data: unknown[] }> };
	ROLLUPS: { put(key: string, value: string, options?: R2PutOptions): Promise<unknown> };
};

/**
 * One UTC day's aggregate counts, written to R2 as `daily/<date>.json`.
 *
 * Raw pings only live as long as Analytics Engine keeps them; these files are
 * what preserves the long-term trend. Rolling windows are computed here because
 * they cannot be rebuilt from daily files once the raw pings are gone.
 */
export type DailyRollup = {
	date: string;
	generatedAt: string;
	dailyActiveUsers: number;
	weeklyActiveUsers: number;
	monthlyActiveUsers: number;
	byPlatform: { os: string; arch: string; users: number }[];
	byVersion: { version: string; users: number }[];
	byCommand: { command: string; users: number }[];
};

/**
 * Returns the UTC day before the cron fired, as `YYYY-MM-DD`.
 *
 * @param scheduledTime - The trigger's scheduled time in Unix milliseconds.
 * @returns The date of the day to aggregate.
 * @example
 * previousUtcDay(Date.UTC(2026, 9, 22, 0, 15)); // '2026-10-21'
 */
export function previousUtcDay(scheduledTime: number): string {
	return isoDate(scheduledTime - DAY_MS);
}

/**
 * Aggregates one UTC day of pings and stores the result in R2, replacing any
 * earlier file for that day so a rerun is safe.
 *
 * @param env - The Worker bindings.
 * @param date - The UTC day to aggregate, as `YYYY-MM-DD`.
 * @param now - When the rollup runs, recorded as `generatedAt`.
 * @returns The rollup that was written.
 */
export async function writeDailyRollup(
	env: RollupBindings,
	date: string,
	now: Date,
): Promise<DailyRollup> {
	const rollup = await buildDailyRollup(env.ANALYTICS, date, now);
	await env.ROLLUPS.put(`daily/${date}.json`, `${JSON.stringify(rollup, null, '\t')}\n`, {
		httpMetadata: { contentType: 'application/json' },
	});
	return rollup;
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

/**
 * A `WHERE` clause covering the `days` UTC days that end at `endMs`.
 *
 * The bounds come from `Date` arithmetic, never from request input, so they are
 * safe to inline.
 */
function window(endMs: number, days: number): string {
	const start = sqlDateTime(endMs - days * DAY_MS);
	const end = sqlDateTime(endMs);
	return `timestamp >= toDateTime('${start}') AND timestamp < toDateTime('${end}')`;
}

async function distinctUsers(
	analytics: RollupBindings['ANALYTICS'],
	where: string,
): Promise<number> {
	const result = await analytics.query({
		query: `SELECT count(DISTINCT index1) AS users FROM ${DATASET} WHERE ${where}`,
	});
	return z.array(countRowSchema).parse(result.data)[0]?.users ?? 0;
}

async function breakdown<Row>(
	analytics: RollupBindings['ANALYTICS'],
	where: string,
	columns: string,
	groupBy: string,
	rowSchema: z.ZodType<Row>,
): Promise<Row[]> {
	const result = await analytics.query({
		query: `SELECT ${columns}, count(DISTINCT index1) AS users FROM ${DATASET} WHERE ${where} GROUP BY ${groupBy} ORDER BY users DESC`,
	});
	return z.array(rowSchema).parse(result.data);
}

function isoDate(ms: number): string {
	return new Date(ms).toISOString().slice(0, 10);
}

function sqlDateTime(ms: number): string {
	return new Date(ms).toISOString().slice(0, 19).replace('T', ' ');
}
