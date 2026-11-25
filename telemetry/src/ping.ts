import type { FromSchema } from 'json-schema-to-ts';
import * as z from 'zod';
import type { JSONSchema } from 'zod/v4/core';

import { pingJsonSchema } from './v1/ping-schema.gen.ts';

const MAX_BODY_BYTES = 1024;

export const headerSchema = z.object({
	'content-type': z.string().startsWith('application/json'),
	'content-length': z.coerce.number().int().nonnegative().max(MAX_BODY_BYTES),
});

export type Ping = FromSchema<typeof pingJsonSchema>;

type Mutable<T> = T extends readonly (infer Item)[]
	? Mutable<Item>[]
	: T extends object
		? { -readonly [Key in keyof T]: Mutable<T[Key]> }
		: T;

// NOTE(2026-11-22): Zod's JSON Schema input requires mutable arrays, while generated `as const` schemas are readonly. `fromJSONSchema` returns an untyped schema, so this restores its output type from the same generated schema.
const mutablePingJsonSchema: JSONSchema.JSONSchema = pingJsonSchema as unknown as Mutable<
	typeof pingJsonSchema
>;

export const pingSchema = z.fromJSONSchema(mutablePingJsonSchema) as z.ZodType<Ping>;

export function recordPing(pings: AnalyticsEngineDataset, ping: Ping): void {
	pings.writeDataPoint({
		indexes: [ping.anonymousId],
		blobs: [ping.event, ping.version, ping.os, ping.arch, ping.command],
	});
}
