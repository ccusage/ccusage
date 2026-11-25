import assert from 'node:assert/strict';
import type { FromSchema } from 'json-schema-to-ts';

import { Hono } from 'hono';
import type { CloudEnv } from 'void';

import { POST } from '../routes/v1/ping.ts';
import { pingJsonSchema } from './v1/ping-schema.gen.ts';

const app = new Hono<CloudEnv>().post('/v1/ping', async (c) => {
	const response = await POST(c);
	if (!(response instanceof Response)) throw new Error('The ping route must return a Response');
	return response;
});

const validPing = {
	event: 'cli_daily_active',
	anonymousId: '4fd4c40cb4140e0e8952ceb2b3502e07',
	version: '20.0.38',
	os: 'macos',
	arch: 'aarch64',
	command: 'codex monthly',
} as const satisfies FromSchema<typeof pingJsonSchema>;

function recordingEnv() {
	const points: AnalyticsEngineDataPoint[] = [];
	const env = {
		PINGS: {
			writeDataPoint: (point?: AnalyticsEngineDataPoint) => {
				if (point !== undefined) {
					points.push(point);
				}
			},
		},
	};
	return { env, points };
}

type PostOptions = {
	contentType?: string;
	contentLength?: string;
};

async function post(
	body: string,
	env: ReturnType<typeof recordingEnv>['env'],
	options: PostOptions = {},
) {
	const headers = {
		'content-type': options.contentType ?? 'application/json',
		'content-length': options.contentLength ?? String(new TextEncoder().encode(body).length),
	} satisfies Record<string, string>;
	return app.request('/v1/ping', { method: 'POST', body, headers }, env);
}

describe('POST /v1/ping', () => {
	it('stores only the documented fields of a valid ping', async () => {
		const { env, points } = recordingEnv();

		const response = await post(JSON.stringify({ ...validPing, ip: '203.0.113.1' }), env);

		assert.equal(response.status, 204);
		assert.deepEqual(points, [
			{
				indexes: [validPing.anonymousId],
				blobs: ['cli_daily_active', '20.0.38', 'macos', 'aarch64', 'codex monthly'],
			},
		]);
	});

	it('rejects malformed pings without storing', async () => {
		const { env, points } = recordingEnv();
		const cases = [
			'{',
			'null',
			JSON.stringify({ ...validPing, event: 'other' }),
			JSON.stringify({ ...validPing, anonymousId: 'not-hex' }),
			JSON.stringify({ ...validPing, anonymousId: validPing.anonymousId.toUpperCase() }),
			JSON.stringify({ ...validPing, command: 'x'.repeat(65) }),
			JSON.stringify({ ...validPing, os: '' }),
			JSON.stringify({ ...validPing, version: 20 }),
			JSON.stringify({ ...validPing, arch: undefined }),
		] as const satisfies readonly string[];

		for (const body of cases) {
			assert.equal((await post(body, env)).status, 400, body);
		}
		assert.equal(points.length, 0);
	});

	it('rejects bad headers before reading the body', async () => {
		const { env, points } = recordingEnv();
		const body = JSON.stringify(validPing);
		const cases = [
			{ contentType: 'text/plain' },
			{ contentLength: '2048' },
			{ contentLength: 'abc' },
		] as const satisfies readonly PostOptions[];

		for (const options of cases) {
			assert.equal((await post(body, env, options)).status, 400, JSON.stringify(options));
		}
		assert.equal(points.length, 0);
	});
});
