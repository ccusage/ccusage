/**
 * The Worker's bindings, as configured in `cloudflare.config.ts`.
 */
export type Env = {
	Bindings: {
		/** Write side of the raw ping dataset. */
		PINGS: AnalyticsEngineDataset;
		/** SQL read side of the same dataset, used by the daily rollup. */
		ANALYTICS: AnalyticsSQLBinding;
		/** Long-term store for the daily aggregate files. */
		ROLLUPS: R2Bucket;
	};
};
