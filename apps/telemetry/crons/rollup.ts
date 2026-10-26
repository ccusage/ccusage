import { defineScheduled } from 'void';

import { previousUtcDay, writeDailyRollup } from '../src/rollup.ts';

// 00:15 UTC leaves a margin for pings still in flight at midnight before the
// previous day is aggregated.
export const cron = '15 0 * * *';

export default defineScheduled(async (controller, env) => {
	await writeDailyRollup(env, previousUtcDay(controller.scheduledTime), new Date());
});
