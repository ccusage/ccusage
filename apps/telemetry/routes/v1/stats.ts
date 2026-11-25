import { defineHandler } from 'void';

import type { PublicStats } from '../../src/rollup.ts';
import { cachePublicStats, readPublicStats } from '../../src/stats.ts';

export const GET = defineHandler(async (c) => {
	const now = Date.now();
	let snapshot: PublicStats | null;
	try {
		snapshot = await readPublicStats(c.env.ROLLUPS, now);
	} catch {
		c.header('Cache-Control', 'no-store');
		return c.json({ error: 'Statistics are temporarily unavailable.' }, 503);
	}
	cachePublicStats(c, snapshot, now);
	c.header('X-Content-Type-Options', 'nosniff');
	return c.json(snapshot ?? { generatedAt: null, days: [] });
});
