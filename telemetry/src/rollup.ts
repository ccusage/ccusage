import * as z from 'zod';

const DATASET = 'events.analyticsEngine.ccusage_telemetry';
const DAY_MS = 24 * 60 * 60 * 1000;
const ROLLUP_MINUTE_UTC = 15;
export const ROLLUP_CRON = `${ROLLUP_MINUTE_UTC} 0 * * *`;
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

export function nextRollupTime(now: number): number {
	const todayRollup = Date.parse(`${isoDate(now)}T00:00:00Z`) + ROLLUP_MINUTE_UTC * 60 * 1000;
	return now < todayRollup ? todayRollup : todayRollup + DAY_MS;
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
		byPlatform: topGroups(rollup.byPlatform),
		byVersion: topGroups(rollup.byVersion),
		...(rollup.samplingObserved === undefined ? {} : { samplingObserved: rollup.samplingObserved }),
	};
}

function topGroups<Row extends { users: number }>(rows: Row[]): Row[] {
	return rows.toSorted((left, right) => right.users - left.users).slice(0, MAX_BREAKDOWN_ROWS);
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

if (import.meta.vitest) {
	const archives = new Map<string, string>(
		Array.from({ length: 120 }, (_, index) => {
			const date = new Date(Date.UTC(2026, 6, 1 + index)).toISOString().slice(0, 10);
			return [
				`daily/${date}.json`,
				JSON.stringify({
					date,
					generatedAt: `${date}T23:59:00Z`,
					dailyActiveUsers: 2,
					weeklyActiveUsers: 4,
					monthlyActiveUsers: 7,
					byPlatform: [{ os: 'linux', arch: 'x86_64', users: 2 }],
					byVersion: [{ version: '20.0.37', users: 2 }],
					byCommand: [{ command: 'daily', users: 2 }],
				} satisfies DailyRollup),
			];
		}),
	);

	const analytics: RollupBindings['ANALYTICS'] = {
		query: async ({ query, params }) => {
			const { start } = Object.fromEntries(Object.entries(params ?? {}));
			if (query.includes('GROUP BY os, arch')) {
				return {
					data: [
						{ os: 'macos', arch: 'aarch64', users: '7', maxSampleInterval: '1' },
						{ os: 'linux', arch: 'x86_64', users: '3', maxSampleInterval: '1' },
					],
				};
			}
			if (query.includes('GROUP BY version')) {
				return { data: [{ version: '20.0.38', users: '10', maxSampleInterval: '1' }] };
			}
			if (query.includes('GROUP BY command')) {
				return {
					data: [
						{ command: 'daily', users: '6', maxSampleInterval: '1' },
						{ command: 'statusline', users: '4', maxSampleInterval: '1' },
					],
				};
			}
			if (start === '2026-09-22T00:00:00.000Z') {
				return { data: [{ users: '42', maxSampleInterval: '1' }] };
			}
			if (start === '2026-10-15T00:00:00.000Z') {
				return { data: [{ users: '25', maxSampleInterval: '1' }] };
			}
			return { data: [{ users: '10', maxSampleInterval: '1' }] };
		},
	};

	describe('previousUtcDay', () => {
		it.each([
			{ scheduledTime: Date.UTC(2026, 9, 22, 0, 15), expected: '2026-10-21' },
			{ scheduledTime: Date.UTC(2026, 0, 1, 0, 15), expected: '2025-12-31' },
		])('returns $expected for a trigger at $scheduledTime', ({ scheduledTime, expected }) => {
			expect(previousUtcDay(scheduledTime)).toBe(expected);
		});
	});

	describe('writeDailyRollup', () => {
		it('archives the daily rollup and publishes a sanitized 90-day snapshot from archives', async () => {
			const values = new Map(
				[...archives].filter(
					([key]) => key >= 'daily/2026-07-23.json' && key <= 'daily/2026-10-20.json',
				),
			);
			values.set(PUBLIC_STATS_KEY, '{');
			const get = vi.fn(
				async (keys: string[]) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
			);
			const put = vi.fn(async (key: string, value: string) => {
				values.set(key, value);
			});

			const rollup = await writeDailyRollup(
				{ ANALYTICS: analytics, ROLLUPS: { get, put } },
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			expect(rollup).toEqual({
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
			expect(get).toHaveBeenCalledOnce();
			expect(get.mock.calls[0]?.[0]).toHaveLength(90);
			expect(get.mock.calls[0]?.[0]).not.toContain(PUBLIC_STATS_KEY);
			expect(put.mock.calls.map(([key]) => key)).toEqual([
				'daily/2026-10-21.json',
				PUBLIC_STATS_KEY,
			]);
			expect(JSON.parse(values.get('daily/2026-10-21.json') ?? '')).toEqual(rollup);

			const published = values.get(PUBLIC_STATS_KEY) ?? '';
			expect(published).not.toContain('anonymousId');
			expect(published).not.toContain('private');
			expect(published).not.toContain('byCommand');
			const snapshot = parsePublicStats(published, '2026-10-22');
			expect(snapshot.generatedAt).toBe('2026-10-22T00:15:00.000Z');
			expect(snapshot.days).toHaveLength(90);
			expect(snapshot.days[0]?.date).toBe('2026-07-24');
			expect(snapshot.days[0]).not.toHaveProperty('samplingObserved');
			expect(snapshot.days.at(-1)).toEqual({
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

		it('publishes a first snapshot without history backfill', async () => {
			const values = new Map<string, string>();
			const get = vi.fn(
				async (keys: string[]) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
			);
			const put = vi.fn(async (key: string, value: string) => {
				values.set(key, value);
			});

			await writeDailyRollup(
				{ ANALYTICS: analytics, ROLLUPS: { get, put } },
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			expect(get.mock.calls[0]?.[0]).toHaveLength(90);
			expect(put.mock.calls.map(([key]) => key)).toEqual([
				'daily/2026-10-21.json',
				PUBLIC_STATS_KEY,
			]);
			const snapshot = parsePublicStats(values.get(PUBLIC_STATS_KEY) ?? '', '2026-10-22');
			expect(snapshot.days.map(({ date }) => date)).toEqual(['2026-10-21']);
		});

		it('bounds legacy archive breakdowns at publication without changing totals or archives', async () => {
			const legacy = {
				date: '2026-10-20',
				generatedAt: '2026-10-21T00:15:00Z',
				dailyActiveUsers: 100,
				weeklyActiveUsers: 200,
				monthlyActiveUsers: 300,
				byPlatform: Array.from({ length: 25 }, (_, index) => ({
					os: `platform-${index}`,
					arch: 'x86_64',
					users: index + 1,
				})),
				byVersion: Array.from({ length: 25 }, (_, index) => ({
					version: `20.0.${index}`,
					users: index + 1,
				})),
				byCommand: [{ command: 'daily', users: 100 }],
			} satisfies DailyRollup;
			const stored = JSON.stringify(legacy);
			const values = new Map([['daily/2026-10-20.json', stored]]);

			await writeDailyRollup(
				{
					ANALYTICS: analytics,
					ROLLUPS: {
						get: async (keys) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
						put: async (key, value) => values.set(key, value),
					},
				},
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			const snapshot = parsePublicStats(values.get(PUBLIC_STATS_KEY) ?? '', '2026-10-22');
			expect(snapshot.days.find(({ date }) => date === legacy.date)).toStrictEqual({
				date: legacy.date,
				generatedAt: legacy.generatedAt,
				dailyActiveUsers: 100,
				weeklyActiveUsers: 200,
				monthlyActiveUsers: 300,
				byPlatform: legacy.byPlatform.toReversed().slice(0, 20),
				byVersion: legacy.byVersion.toReversed().slice(0, 20),
			});
			expect(values.get('daily/2026-10-20.json')).toBe(stored);
		});

		it('replaces an older day without dropping newer archive data', async () => {
			const values = new Map(
				[...archives].filter(
					([key]) => key >= 'daily/2026-07-24.json' && key <= 'daily/2026-10-21.json',
				),
			);
			const put = vi.fn(async (key: string, value: string) => {
				values.set(key, value);
			});

			await writeDailyRollup(
				{
					ANALYTICS: analytics,
					ROLLUPS: {
						get: async (keys) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
						put,
					},
				},
				'2026-10-20',
				new Date('2026-10-22T00:15:00Z'),
			);

			const snapshot = parsePublicStats(values.get(PUBLIC_STATS_KEY) ?? '', '2026-10-22');
			expect(snapshot.days).toHaveLength(90);
			expect(snapshot.days.at(-1)?.date).toBe('2026-10-21');
			expect(snapshot.generatedAt).toBe('2026-10-21T23:59:00Z');
			expect(snapshot.days.find(({ date }) => date === '2026-10-20')).toMatchObject({
				dailyActiveUsers: 10,
				samplingObserved: false,
			});
			expect(put).toHaveBeenCalledTimes(2);
		});

		it('fails closed when an authoritative archive is corrupt', async () => {
			const values = new Map(
				[...archives].filter(
					([key]) => key >= 'daily/2026-07-24.json' && key <= 'daily/2026-10-20.json',
				),
			);
			values.set('daily/2026-10-01.json', '{');
			values.set(PUBLIC_STATS_KEY, '{');
			const put = vi.fn(async (key: string, value: string) => {
				values.set(key, value);
			});

			await expect(
				writeDailyRollup(
					{
						ANALYTICS: analytics,
						ROLLUPS: {
							get: async (keys) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
							put,
						},
					},
					'2026-10-21',
					new Date('2026-10-22T00:15:00Z'),
				),
			).rejects.toThrow();
			expect(put.mock.calls.map(([key]) => key)).toEqual(['daily/2026-10-21.json']);
			expect(values.get(PUBLIC_STATS_KEY)).toBe('{');
		});

		it('bounds every query to whole UTC days ending at midnight after the date', async () => {
			const query = vi.fn((request: AnalyticsSQLQuery) => analytics.query(request));

			await writeDailyRollup(
				{
					ANALYTICS: { query },
					ROLLUPS: {
						get: async (keys) => new Map(keys.map((key) => [key, null])),
						put: async () => {},
					},
				},
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			const queries = query.mock.calls.map(([request]) => request.query);
			expect(queries).toEqual(
				Array.from({ length: 6 }, () =>
					expect.stringMatching(
						/count\(DISTINCT index1\) AS users, approx_max\(sampleInterval\) AS maxSampleInterval FROM events\.analyticsEngine\.ccusage_telemetry WHERE timestamp >= \$start AND timestamp < \$end/,
					),
				),
			);
			expect(queries.join('\n')).not.toContain('toDateTime');
			expect(new Set(query.mock.calls.map(([request]) => JSON.stringify(request.params)))).toEqual(
				new Set([
					JSON.stringify({ start: '2026-10-21T00:00:00.000Z', end: '2026-10-22T00:00:00.000Z' }),
					JSON.stringify({ start: '2026-10-15T00:00:00.000Z', end: '2026-10-22T00:00:00.000Z' }),
					JSON.stringify({ start: '2026-09-22T00:00:00.000Z', end: '2026-10-22T00:00:00.000Z' }),
				]),
			);
		});

		it('marks observed sampling without scaling distinct-user counts', async () => {
			const rollup = await writeDailyRollup(
				{
					ANALYTICS: {
						query: async (request) =>
							request.query.includes('GROUP BY version')
								? { data: [{ version: '20.0.38', users: '10', maxSampleInterval: '3' }] }
								: analytics.query(request),
					},
					ROLLUPS: {
						get: async (keys) => new Map(keys.map((key) => [key, null])),
						put: async () => {},
					},
				},
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			expect(rollup.samplingObserved).toBe(true);
			expect(rollup.dailyActiveUsers).toBe(10);
			expect(rollup.byVersion).toEqual([{ version: '20.0.38', users: 10 }]);
		});

		it('limits all breakdown queries while preserving full distinct-user totals', async () => {
			const query = vi.fn((request: AnalyticsSQLQuery) => analytics.query(request));

			const rollup = await writeDailyRollup(
				{
					ANALYTICS: { query },
					ROLLUPS: {
						get: async (keys) => new Map(keys.map((key) => [key, null])),
						put: async () => {},
					},
				},
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			const queries = query.mock.calls.map(([request]) => request.query);
			expect(queries.filter((sql) => sql.includes('GROUP BY'))).toEqual(
				Array.from({ length: 3 }, () => expect.stringMatching(/ORDER BY users DESC LIMIT 20$/)),
			);
			expect(queries.filter((sql) => !sql.includes('GROUP BY'))).toEqual(
				Array.from({ length: 3 }, () => expect.not.stringContaining('LIMIT')),
			);
			expect([
				rollup.dailyActiveUsers,
				rollup.weeklyActiveUsers,
				rollup.monthlyActiveUsers,
			]).toEqual([10, 25, 42]);
		});

		it('does not publish zeros when Analytics Engine returns no count row', async () => {
			const put = vi.fn(async () => {});

			await expect(
				writeDailyRollup(
					{
						ANALYTICS: { query: async () => ({ data: [] }) },
						ROLLUPS: { get: async (keys) => new Map(keys.map((key) => [key, null])), put },
					},
					'2026-10-21',
					new Date('2026-10-22T00:15:00Z'),
				),
			).rejects.toThrow();
			expect(put).not.toHaveBeenCalled();
		});

		it('rejects rows that do not match the expected shape instead of writing them', async () => {
			const put = vi.fn(async () => {});

			await expect(
				writeDailyRollup(
					{
						ANALYTICS: { query: async () => ({ data: [{ users: 'many' }] }) },
						ROLLUPS: { get: async (keys) => new Map(keys.map((key) => [key, null])), put },
					},
					'2026-10-21',
					new Date('2026-10-22T00:15:00Z'),
				),
			).rejects.toThrow();
			expect(put).not.toHaveBeenCalled();
		});
	});

	describe('writeRecentRollups', () => {
		it('recovers recent missed dates and rebuilds a corrupt public snapshot within 23 operations', async () => {
			const values = new Map(
				[...archives].filter(
					([key]) => key >= 'daily/2026-07-24.json' && key <= 'daily/2026-10-18.json',
				),
			);
			values.set(PUBLIC_STATS_KEY, '{');
			const query = vi.fn((request: AnalyticsSQLQuery) => analytics.query(request));
			const get = vi.fn(
				async (keys: string[]) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
			);
			const put = vi.fn(async (key: string, value: string) => {
				values.set(key, value);
			});

			const rollups = await writeRecentRollups(
				{ ANALYTICS: { query }, ROLLUPS: { get, put } },
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			expect(rollups.map(({ date }) => date)).toEqual(['2026-10-19', '2026-10-20', '2026-10-21']);
			expect(query).toHaveBeenCalledTimes(18);
			expect(get).toHaveBeenCalledOnce();
			expect(get.mock.calls[0]?.[0]).toHaveLength(90);
			expect(put.mock.calls.map(([key]) => key)).toEqual([
				'daily/2026-10-19.json',
				'daily/2026-10-20.json',
				'daily/2026-10-21.json',
				PUBLIC_STATS_KEY,
			]);
			const snapshot = parsePublicStats(values.get(PUBLIC_STATS_KEY) ?? '', '2026-10-22');
			expect(snapshot.days).toHaveLength(90);
			expect(snapshot.days.slice(-3).map(({ date }) => date)).toEqual([
				'2026-10-19',
				'2026-10-20',
				'2026-10-21',
			]);
			expect(snapshot.generatedAt).toBe('2026-10-22T00:15:00.000Z');
		});

		it('keeps genuine archive gaps instead of creating zero-valued days', async () => {
			const values = new Map(
				[...archives].filter(
					([key]) =>
						key >= 'daily/2026-07-24.json' &&
						key <= 'daily/2026-10-18.json' &&
						key !== 'daily/2026-10-05.json',
				),
			);

			await writeRecentRollups(
				{
					ANALYTICS: analytics,
					ROLLUPS: {
						get: async (keys) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
						put: async (key, value) => values.set(key, value),
					},
				},
				'2026-10-21',
				new Date('2026-10-22T00:15:00Z'),
			);

			const snapshot = parsePublicStats(values.get(PUBLIC_STATS_KEY) ?? '', '2026-10-22');
			expect(snapshot.days).toHaveLength(89);
			expect(snapshot.days.map(({ date }) => date)).not.toContain('2026-10-05');
			expect(snapshot.days.map(({ dailyActiveUsers }) => dailyActiveUsers)).not.toContain(0);
		});

		it('fails closed if an authoritative archive cannot be parsed', async () => {
			const values = new Map(
				[...archives].filter(
					([key]) => key >= 'daily/2026-07-24.json' && key <= 'daily/2026-10-18.json',
				),
			);
			values.set('daily/2026-10-05.json', '{');
			values.set(PUBLIC_STATS_KEY, '{');
			const put = vi.fn(async (key: string, value: string) => {
				values.set(key, value);
			});

			await expect(
				writeRecentRollups(
					{
						ANALYTICS: analytics,
						ROLLUPS: {
							get: async (keys) => new Map(keys.map((key) => [key, values.get(key) ?? null])),
							put,
						},
					},
					'2026-10-21',
					new Date('2026-10-22T00:15:00Z'),
				),
			).rejects.toThrow();
			expect(put.mock.calls.map(([key]) => key)).toEqual([
				'daily/2026-10-19.json',
				'daily/2026-10-20.json',
				'daily/2026-10-21.json',
			]);
			expect(values.get(PUBLIC_STATS_KEY)).toBe('{');
		});
	});
}
