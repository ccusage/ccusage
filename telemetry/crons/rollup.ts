import { defineScheduled } from 'void';

import { previousUtcDay, writeRecentRollups } from '../src/rollup.ts';
import { purgePublicStatsCache } from '../src/stats.ts';

export const cron = '15 0 * * *';

export default defineScheduled(async (controller, env, ctx) => {
	await writeRecentRollups(env, previousUtcDay(controller.scheduledTime), new Date());
	await purgePublicStatsCache(ctx.cache);
});
