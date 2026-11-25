import { defineConfig } from 'void/config';

export default defineConfig({
	worker: { compatibility_date: '2026-10-01' },
	inference: { bindings: { db: false, kv: false, storage: false, ai: false } },
	cloudflare: {
		name: 'ccusage-telemetry',
		account_id: 'e721638321cf56dac84472ad22a161ed',
		routes: [{ pattern: 'telemetry.ccusage.com', custom_domain: true }],
		cache: { enabled: true },
		observability: { enabled: false },
		analytics_engine_datasets: [{ binding: 'PINGS', dataset: 'ccusage_telemetry' }],
		analytics: { binding: 'ANALYTICS' },
		kv_namespaces: [{ binding: 'ROLLUPS', id: 'f6ff6aa494c5449092819437316fde2d' }],
		version_metadata: { binding: 'VERSION' },
	},
});
