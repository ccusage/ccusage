import { defineConfig } from 'void/config';

export default defineConfig({
	worker: {
		compatibility_date: '2026-10-01',
	},
	// Void only infers its own resources (DB, KV, STORAGE, AI, queues), so the
	// telemetry bindings pass through in wrangler's format.
	cloudflare: {
		name: 'ccusage-telemetry',
		routes: [{ pattern: 'telemetry.ccusage.com', custom_domain: true }],
		// Workers Logs would keep request metadata such as the client IP, which the
		// telemetry guide promises is never stored.
		observability: { enabled: false },
		analytics_engine_datasets: [{ binding: 'PINGS', dataset: 'ccusage_telemetry' }],
		// Remote-only: running the rollup locally needs a Cloudflare login,
		// since Analytics SQL has no local simulation.
		analytics: { binding: 'ANALYTICS' },
		r2_buckets: [{ binding: 'ROLLUPS', bucket_name: 'ccusage-telemetry-rollups' }],
	},
});
