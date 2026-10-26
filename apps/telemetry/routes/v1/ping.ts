import { defineHandler } from 'void';

import { app } from '../../src/index.ts';

// Void routes by file, but the endpoint stays a plain Hono app in `src/` so its
// typed validators and tests do not depend on Void; this file only hands over.
export const POST = defineHandler((c) => app.fetch(c.req.raw, c.env, c.executionCtx));
