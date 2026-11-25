import { zValidator } from '@hono/zod-validator';
import { defineHandler } from 'void';

import { headerSchema, pingSchema, recordPing } from '../../src/ping.ts';

export const POST = defineHandler(
	zValidator('header', headerSchema),
	defineHandler.withValidator({ body: pingSchema })((c, { body }) => {
		recordPing(c.env.PINGS, body);
		return c.body(null, 204);
	}),
);
