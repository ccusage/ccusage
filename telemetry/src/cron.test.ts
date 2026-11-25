import assert from 'node:assert/strict';

import { cron } from '../crons/rollup.ts';
import { ROLLUP_CRON } from './rollup.ts';

describe('rollup cron', () => {
	it('runs at the time the edge cache lifetime expects', () => {
		assert.equal(cron, ROLLUP_CRON);
	});
});
