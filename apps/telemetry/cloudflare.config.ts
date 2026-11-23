import { bindings, defineConfig, triggers } from 'cf/config';

import * as entrypoint from './src/index.ts' with { type: 'cf-worker' };

export default defineConfig({
	accountId: 'e721638321cf56dac84472ad22a161ed',
	worker: {
		name: 'ccusage-telemetry',
		compatibilityDate: '2026-10-01',
		entrypoint,
		assets: { runWorkerFirst: ['/'] },
		domains: ['telemetry.ccusage.com'],
		triggers: [triggers.scheduled({ schedule: '15 0 * * *' })],
		observability: { enabled: false },
		env: {
			ASSETS: bindings.assets(),
			PINGS: bindings.analyticsEngineDataset({ name: 'ccusage_telemetry' }),
			ANALYTICS: bindings.analyticsSql(),
			ROLLUPS: bindings.kv({ id: 'f6ff6aa494c5449092819437316fde2d' }),
		},
	},
});
