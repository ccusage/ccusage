/**
 * The Worker's bindings, as configured in `void.config.ts`.
 */
type Bindings = {
	/** Write side of the raw ping dataset. */
	PINGS: AnalyticsEngineDataset;
	/** SQL read side of the same dataset, used by the daily rollup. */
	ANALYTICS: AnalyticsSQLBinding;
	/** Long-term store for the daily aggregate files. */
	ROLLUPS: R2Bucket;
};

export type Env = { Bindings: Bindings };

// Void only generates env types for the resources it provisions itself, so the
// passthrough bindings are declared on the interface its handlers read `env` from.
declare global {
	interface VoidGeneratedEnvBindings extends Bindings {}
}
