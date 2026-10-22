/**
 * The Worker's bindings, as configured in `cloudflare.config.ts`.
 */
export type Env = {
	Bindings: {
		PINGS: AnalyticsEngineDataset;
	};
};
