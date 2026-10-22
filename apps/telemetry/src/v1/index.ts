import { Hono } from 'hono';

import type { Env } from '../env.ts';
import { ping } from './ping.ts';

export const v1 = new Hono<Env>().route('/ping', ping);
