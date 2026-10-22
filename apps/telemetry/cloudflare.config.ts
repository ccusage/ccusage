import { bindings, defineConfig, triggers } from 'cf/config';

import * as entrypoint from './src/index.ts' with { type: 'cf-worker' };

export default defineConfig({
	worker: {
		name: 'ccusage-telemetry',
		compatibilityDate: '2026-10-01',
		entrypoint,
		domains: ['telemetry.ccusage.com'],
		// 00:15 UTC leaves a margin for pings still in flight at midnight before
		// the previous day is aggregated.
		triggers: [triggers.scheduled({ schedule: '15 0 * * *' })],
		// Workers Logs would keep request metadata such as the client IP, which the
		// telemetry guide promises is never stored.
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
