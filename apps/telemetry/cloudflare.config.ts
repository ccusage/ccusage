import { bindings, defineConfig, triggers } from 'cf/config';

import * as entrypoint from './src/index.ts' with { type: 'cf-worker' };

export default defineConfig({
	accountId: 'e721638321cf56dac84472ad22a161ed',
	worker: {
		name: 'ccusage-telemetry',
		compatibilityDate: '2026-10-01',
		entrypoint,
		domains: ['telemetry.ccusage.com'],
		// 00:15 UTC leaves a margin for pings still in flight at midnight before
		// the previous day is aggregated.
		triggers: [triggers.scheduled({ schedule: '15 0 * * *' })],
		// Request logs would retain metadata outside the documented ping fields.
		observability: { enabled: false },
		env: {
			PINGS: bindings.analyticsEngineDataset({ name: 'ccusage_telemetry' }),
			// Remote-only: there is no local simulation, so `cf dev` needs a Cloudflare
			// login. In exchange the rollup reads the dataset without an API token secret.
			ANALYTICS: bindings.analytics(),
			ROLLUPS: bindings.r2({ name: 'ccusage-telemetry-rollups' }),
		},
	},
});
