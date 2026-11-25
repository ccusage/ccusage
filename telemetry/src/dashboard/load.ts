import type { Context } from 'hono';
import { Hono } from 'hono';
import type { CloudEnv } from 'void';

import type { PublicStats } from '../rollup.ts';
import { cachePublicStats, readPublicStats } from '../stats.ts';

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

if (import.meta.vitest) {
	const app = new Hono<CloudEnv>().get('/', async (c) => c.json(await loadDashboard(c)));

	describe('loadDashboard', () => {
		beforeEach(() => {
			vi.useFakeTimers({ now: Date.parse('2026-11-25T12:00:00Z'), toFake: ['Date'] });
		});
		afterEach(() => {
			vi.useRealTimers();
		});

		it('loads the snapshot with restrictive headers and the same edge lifetime as the API', async () => {
			const stored = {
				generatedAt: '2026-11-25T00:15:42Z',
				days: [
					{
						date: '2026-11-24',
						generatedAt: '2026-11-25T00:15:42Z',
						dailyActiveUsers: 3,
						weeklyActiveUsers: 8,
						monthlyActiveUsers: 12,
						byPlatform: [{ os: 'linux', arch: 'x86_64', users: 3 }],
						byVersion: [{ version: '20.1.0', users: 3 }],
						byCommand: [{ command: 'daily', users: 3 }],
					},
				],
			};

			const response = await app.request(
				'/',
				{},
				{ ROLLUPS: { get: async () => JSON.stringify(stored) } },
			);

			expect(response.status).toBe(200);
			expect(response.headers.get('content-security-policy')).toContain("script-src 'self'");
			expect(response.headers.get('x-content-type-options')).toBe('nosniff');
			expect(response.headers.get('referrer-policy')).toBe('no-referrer');
			expect(response.headers.get('cloudflare-cdn-cache-control')).toBe('public, max-age=44100');
			expect(response.headers.get('cache-tag')).toBe('public-stats');
			expect(await response.json()).toEqual({
				stats: {
					generatedAt: '2026-11-25T00:15:42Z',
					days: [
						{
							date: '2026-11-24',
							generatedAt: '2026-11-25T00:15:42Z',
							dailyActiveUsers: 3,
							weeklyActiveUsers: 8,
							monthlyActiveUsers: 12,
							byPlatform: [{ os: 'linux', arch: 'x86_64', users: 3 }],
							byVersion: [{ version: '20.1.0', users: 3 }],
						},
					],
				},
				unavailable: false,
				now: Date.parse('2026-11-25T12:00:00Z'),
			});
		});

		it('renders an uncacheable unavailable state when the snapshot cannot be read', async () => {
			const response = await app.request('/', {}, { ROLLUPS: { get: async () => '{' } });

			expect(response.status).toBe(200);
			expect(response.headers.get('cache-control')).toBe('no-store');
			expect(await response.json()).toEqual({
				stats: null,
				unavailable: true,
				now: Date.parse('2026-11-25T12:00:00Z'),
			});
		});
	});
}
