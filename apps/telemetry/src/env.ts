export type Env = {
	Bindings: {
		ASSETS: Fetcher;
		PINGS: AnalyticsEngineDataset;
		ANALYTICS: AnalyticsSQLBinding;
		ROLLUPS: KVNamespace;
	};
};
