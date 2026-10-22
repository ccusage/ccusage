import { bindings, defineConfig } from 'cf/config';

import * as entrypoint from './src/index.ts' with { type: 'cf-worker' };

export default defineConfig({
	worker: {
		name: 'ccusage-telemetry',
		compatibilityDate: '2026-10-01',
		entrypoint,
		domains: ['telemetry.ccusage.com'],
		// Workers Logs would keep request metadata such as the client IP, which the
		// telemetry guide promises is never stored.
		observability: { enabled: false },
		env: {
			PINGS: bindings.analyticsEngineDataset({ name: 'ccusage_telemetry' }),
		},
	},
});
