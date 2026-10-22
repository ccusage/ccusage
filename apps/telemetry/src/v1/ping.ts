import { zValidator } from '@hono/zod-validator';
import { Hono } from 'hono';
import type { FromSchema } from 'json-schema-to-ts';
import { z } from 'zod';
import type { JSONSchema } from 'zod/v4/core';

import type { Env } from '../env.ts';
import { pingJsonSchema } from './ping-schema.gen.ts';

// A real ping is about 200 bytes; anything far larger is not from the CLI.
const MAX_BODY_BYTES = 1024;

const headerSchema = z.object({
	'content-type': z.string().startsWith('application/json'),
	'content-length': z.coerce.number().int().nonnegative().max(MAX_BODY_BYTES),
});

/**
 * The daily ping, derived from the Rust `Ping` type in
 * `rust/crates/ccusage-telemetry` through the generated JSON Schema.
 */
type Ping = FromSchema<typeof pingJsonSchema>;

/** Strips `readonly` from an `as const` literal, recursively. */
type Mutable<T> = T extends readonly (infer Item)[]
	? Mutable<Item>[]
	: T extends object
		? { -readonly [Key in keyof T]: Mutable<T[Key]> }
		: T;

// zod types its JSON Schema input with mutable arrays, which TypeScript will not
// convert a readonly `as const` tuple into even with `as`. The annotation still
// checks the mutable shape against zod's JSON Schema type, and nothing mutates it.
const mutablePingJsonSchema: JSONSchema.JSONSchema = pingJsonSchema as unknown as Mutable<
	typeof pingJsonSchema
>;

// `fromJSONSchema` returns an untyped schema; its output type is restored from
// the same object the validator is built from.
const pingSchema = z.fromJSONSchema(mutablePingJsonSchema) as z.ZodType<Ping>;

export const ping = new Hono<Env>().post(
	'/',
	zValidator('header', headerSchema),
	zValidator('json', pingSchema),
	(c) => {
		const ping = c.req.valid('json');
		// Only the documented fields are stored, even if a newer CLI sends more.
		// The client IP, headers, and Cloudflare's request metadata are left out,
		// as the telemetry guide promises.
		c.env.PINGS.writeDataPoint({
			indexes: [ping.anonymousId],
			blobs: [ping.event, ping.version, ping.os, ping.arch, ping.command],
		});
		return c.body(null, 204);
	},
);
