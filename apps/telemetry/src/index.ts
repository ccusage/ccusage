import { Hono } from 'hono';

import type { Env } from './env.ts';
import { dashboardHtml, dashboardScript } from './dashboard.ts';
import { previousUtcDay, writeDailyRollup } from './rollup.ts';
import { stats } from './stats.ts';
import { v1 } from './v1/index.ts';

export const app = new Hono<Env>()
	.get('/', (c) => {
		c.header(
			'Content-Security-Policy',
			"default-src 'none'; script-src 'self'; style-src 'unsafe-inline'; connect-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'",
		);
		c.header('Cache-Control', 'public, max-age=300');
		c.header('X-Content-Type-Options', 'nosniff');
		c.header('Referrer-Policy', 'no-referrer');
		return c.html(dashboardHtml);
	})
	.get('/dashboard.js', (c) => {
		c.header('Content-Type', 'text/javascript; charset=utf-8');
		c.header('Cache-Control', 'public, max-age=300');
		c.header('X-Content-Type-Options', 'nosniff');
		return c.body(dashboardScript);
	})
	.route('/v1', stats)
	.route('/v1', v1);

export default {
	fetch: app.fetch,
	scheduled(controller, env, ctx) {
		ctx.waitUntil(writeDailyRollup(env, previousUtcDay(controller.scheduledTime), new Date()));
	},
} satisfies ExportedHandler<Env['Bindings']>;
