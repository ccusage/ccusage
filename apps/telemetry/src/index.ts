import { Hono } from 'hono';

import type { Env } from './env.ts';
import { v1 } from './v1/index.ts';

export const app = new Hono<Env>().route('/v1', v1);

export default app;
