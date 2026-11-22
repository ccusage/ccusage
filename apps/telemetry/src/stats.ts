import { Hono } from 'hono';
import * as z from 'zod';

import type { Env } from './env.ts';

const count = z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER);
const daySchema = z.object({
	date: z.iso.date(),
	generatedAt: z.iso.datetime({ offset: true }),
	dailyActiveUsers: count,
	weeklyActiveUsers: count,
	monthlyActiveUsers: count,
	byPlatform: z.array(z.object({ os: z.string(), arch: z.string(), users: count })),
	byVersion: z.array(z.object({ version: z.string(), users: count })),
});
const DAY_MS = 86_400_000;

export const stats = new Hono<Env>().get('/stats', async (c) => {
	// Ignore query strings so arbitrary URLs cannot multiply cache entries.
	const key = new Request(new URL('/v1/stats', c.req.url));
	const cache = typeof caches === 'undefined' ? undefined : caches.default;
	try {
		const cached = await cache?.match(key);
		if (cached) return cached;
		const today = Date.parse(`${new Date().toISOString().slice(0, 10)}T00:00:00Z`);
		const days: z.infer<typeof daySchema>[] = [];
		// Only completed UTC days are public. Missing files remain gaps, not zeroes.
		for (let offset = 90; offset > 0; offset -= 6) {
			const batch = await Promise.all(
				Array.from({ length: 6 }, async (_, index) => {
					const date = new Date(today - (offset - index) * DAY_MS).toISOString().slice(0, 10);
					const object = await c.env.ROLLUPS.get(`daily/${date}.json`);
					if (!object) return null;
					const day = daySchema.parse(await object.json());
					if (day.date !== date) throw new Error('Aggregate date mismatch');
					return day;
				}),
			);
			for (const day of batch) if (day) days.push(day);
		}
		const latest = days.at(-1);
		const response = c.json({ generatedAt: latest?.generatedAt ?? null, days });
		response.headers.set('Cache-Control', 'public, max-age=300, s-maxage=3600');
		response.headers.set('X-Content-Type-Options', 'nosniff');
		await cache?.put(key, response.clone());
		return response;
	} catch {
		c.header('Cache-Control', 'no-store');
		return c.json({ error: 'Statistics are temporarily unavailable.' }, 503);
	}
});
