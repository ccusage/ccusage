import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import type { FromSchema } from 'json-schema-to-ts';

import { app } from '../index.ts';
import { pingJsonSchema } from './ping-schema.gen.ts';

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

void describe('POST /v1/ping', () => {
	void it('stores only the documented fields of a valid ping', async () => {
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

	void it('rejects malformed pings without storing', async () => {
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

	void it('rejects bad headers before reading the body', async () => {
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

	void it('serves nothing outside POST /v1/ping', async () => {
		const { env } = recordingEnv();

		assert.equal((await app.request('/v1/ping', { method: 'GET' }, env)).status, 404);
		assert.equal((await app.request('/ping', { method: 'POST' }, env)).status, 404);
	});
});
