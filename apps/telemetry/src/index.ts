import { Hono } from 'hono';

import type { Env } from './env.ts';
import { previousUtcDay, writeDailyRollup } from './rollup.ts';
import { v1 } from './v1/index.ts';

export const app = new Hono<Env>().route('/v1', v1);

export default {
	fetch: app.fetch,
	scheduled(controller, env, ctx) {
		ctx.waitUntil(writeDailyRollup(env, previousUtcDay(controller.scheduledTime), new Date()));
	},
} satisfies ExportedHandler<Env['Bindings']>;
