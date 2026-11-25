import assert from 'node:assert/strict';
import { describe, it } from 'node:test';

import { cron } from '../crons/rollup.ts';
import { ROLLUP_CRON } from './rollup.ts';

void describe('rollup cron', () => {
	void it('runs at the time the edge cache lifetime expects', () => {
		assert.equal(cron, ROLLUP_CRON);
	});
});
