import type { Context } from 'hono';
import { defineHandler, defineHead } from 'void';

import type { PublicStats } from '../src/rollup.ts';
import { cachePublicStats, readPublicStats } from '../src/stats.ts';

export type Props = { stats: PublicStats | null; unavailable: boolean; now: number };

const CONTENT_SECURITY_POLICY =
	"default-src 'none'; script-src 'self'; style-src 'self' 'unsafe-inline'; connect-src 'self'; img-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'";

export async function loadDashboard(c: Context): Promise<Props> {
	const now = Date.now();
	c.header('Content-Security-Policy', CONTENT_SECURITY_POLICY);
	c.header('X-Content-Type-Options', 'nosniff');
	c.header('Referrer-Policy', 'no-referrer');
	try {
		const stats = await readPublicStats(c.env.ROLLUPS, now);
		cachePublicStats(c, stats, now);
		return { stats, unavailable: false, now };
	} catch {
		c.header('Cache-Control', 'no-store');
		return { stats: null, unavailable: true, now };
	}
}

export const loader = defineHandler(loadDashboard);

export const head = defineHead<Props>(() => ({
	title: 'Usage statistics | ccusage',
	htmlAttrs: { lang: 'en' },
	meta: [
		{ charset: 'utf-8' },
		{ name: 'viewport', content: 'width=device-width,initial-scale=1' },
		{
			name: 'description',
			content:
				'Public ccusage activity statistics, aggregated daily without publishing installation identifiers.',
		},
	],
}));
