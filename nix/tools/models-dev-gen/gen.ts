import { rejectEscapedFileNames, restoreCatalogIdentifiers } from './catalog.ts';
import { generateCatalog } from './packages/core/src/generate.ts';
import {
	buildModelsDevCatalogIndex,
	formatDuplicateModelsDevPricingKeyWarning,
	isPriceableModelsDevCost,
	isTierVariantOfAuthoredModel,
	isTokenPricedModel,
	type ModelsDevCostTier,
	normalizeModelId,
	selectLongContextTier,
	isUnversionedModelId,
	type ModelsDevPricingCandidate,
	modelsDevProviderTrust,
	modelsDevCatalogRulesArtifact,
	selectModelsDevPricingKey,
	shouldReplaceModelsDevPricingCandidate,
} from './compact.ts';

type Cost = {
	input?: number | null;
	output?: number | null;
	cache_read?: number | null;
	cache_write?: number | null;
	tiers?: ModelsDevCostTier[];
};
type Model = {
	id?: string;
	cost?: Cost;
	limit?: { context?: number | null };
	modalities?: { input?: readonly string[]; output?: readonly string[] };
	experimental?: { modes?: Readonly<Record<string, unknown>> };
};
type ModelMetadata = {
	id?: string;
	modalities?: { input?: readonly string[]; output?: readonly string[] };
};
type Provider = { id?: string; models?: Record<string, Model> };
type EmbeddedModel = {
	cost: Cost;
	provider: string;
	limit?: { context: number };
	exactOnly?: true;
};
rejectEscapedFileNames(['models', 'providers']);
const catalog = (await generateCatalog('.')) as {
	models: Record<string, ModelMetadata>;
	providers: Record<string, Provider>;
};

const models = restoreCatalogIdentifiers(catalog.models);
const providers = Object.fromEntries(
	Object.entries(restoreCatalogIdentifiers(catalog.providers)).map(([id, provider]) => [
		id,
		{
			...provider,
			models: restoreCatalogIdentifiers(provider.models ?? {}),
		},
	]),
);

const catalogIndex = buildModelsDevCatalogIndex(models, providers);

const selected: Record<
	string,
	{ candidate: ModelsDevPricingCandidate; entry: EmbeddedModel; pricingKey: string }
> = {};
for (const [providerId, provider] of sortedEntries(providers)) {
	const sourceProviderId = provider.id ?? providerId;
	for (const [modelId, model] of sortedEntries(provider.models ?? {})) {
		const trust = modelsDevProviderTrust({
			providerId: sourceProviderId,
			sourceModelId: modelId,
			index: catalogIndex,
		});
		const cost = model.cost ?? {};
		if (
			!isPriceableModelsDevCost(cost) ||
			!isTokenPricedModel({
				sourceModelId: modelId,
				modalities: model.modalities,
				index: catalogIndex,
			})
		) {
			continue;
		}
		const pricingKey = selectModelsDevPricingKey(modelId, model.id);
		const longContextTier = selectLongContextTier(cost.tiers);
		const entry: EmbeddedModel = {
			provider: sourceProviderId,
			cost: {
				input: cost.input,
				output: cost.output,
				...(cost.cache_read != null ? { cache_read: cost.cache_read } : {}),
				...(cost.cache_write != null ? { cache_write: cost.cache_write } : {}),
				...(longContextTier != null ? { tiers: [longContextTier] } : {}),
			},
		};
		if (model.limit?.context != null) {
			entry.limit = { context: model.limit.context };
		}
		if (isTierVariantOfAuthoredModel(modelId, catalogIndex) || isUnversionedModelId(pricingKey)) {
			entry.exactOnly = true;
		}
		const candidate: ModelsDevPricingCandidate = {
			sourceProviderId,
			sourceModelId: modelId,
			trust,
			hasLongContextTier: longContextTier != null,
			hasContextLimit: entry.limit != null,
			hasExplicitCacheRead: cost.cache_read != null,
			hasExplicitCacheWrite: cost.cache_write != null,
		};
		const normalizedKey = normalizeModelId(pricingKey);
		const existing = selected[normalizedKey];
		if (existing != null) {
			const replacement = shouldReplaceModelsDevPricingCandidate(existing.candidate, candidate);
			if (existing.candidate.trust === candidate.trust) {
				console.warn(
					formatDuplicateModelsDevPricingKeyWarning({
						pricingKey,
						sourceModelId: replacement ? existing.candidate.sourceModelId : candidate.sourceModelId,
					}),
				);
			}
			if (!replacement) {
				continue;
			}
		}
		selected[normalizedKey] = { candidate, entry, pricingKey };
	}
}

const out = Object.fromEntries(
	Object.values(selected).map(({ pricingKey, entry }) => [pricingKey, entry]),
);

const sortObject = (value: unknown): unknown => {
	if (Array.isArray(value)) {
		return value.map(sortObject);
	}
	if (value != null && typeof value === 'object') {
		return Object.fromEntries(
			Object.keys(value as Record<string, unknown>)
				.sort()
				.map((key) => [key, sortObject((value as Record<string, unknown>)[key])]),
		);
	}
	return value;
};

function sortedEntries<T>(value: Record<string, T>): Array<[string, T]> {
	return Object.entries(value).sort(([left], [right]) =>
		left < right ? -1 : left > right ? 1 : 0,
	);
}

const outfile = process.env.OUTFILE;
if (outfile == null || outfile.length === 0) {
	throw new Error('OUTFILE environment variable is required');
}

await Bun.write(outfile, `${JSON.stringify(sortObject(out), null, 2)}\n`);

const catalogRulesOutfile = process.env.CATALOG_RULES_OUTFILE;
if (catalogRulesOutfile != null && catalogRulesOutfile.length > 0) {
	await Bun.write(
		catalogRulesOutfile,
		`${JSON.stringify(modelsDevCatalogRulesArtifact(catalogIndex), null, 2)}\n`,
	);
}
