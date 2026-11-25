declare global {
	interface VoidGeneratedEnvBindings {
		ANALYTICS: AnalyticsSQLBinding;
		PINGS: AnalyticsEngineDataset;
		ROLLUPS: KVNamespace;
		VERSION: WorkerVersionMetadata;
	}
}

export {};
