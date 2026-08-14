/**
 * Generate the committed models.dev pricing snapshot.
 *
 * models.dev ships per-model TOML sources rather than a prebuilt catalog, so we
 * reuse its own `generateCatalog` routine (the same code that backs
 * https://models.dev/api.json) and then compact the result down to the few
 * pricing fields ccusage consumes. Every text model a trusted catalog publishes
 * is kept, and each pricing key takes the most trustworthy catalog's rates (see
 * `compact.ts`). The embedded output is a flat map keyed by runtime model id.
 * The output is committed to the repository and embedded at build time, so
 * every platform ships the identical, pinned data without any build-time
 * network access. The same pinned catalog also generates the Codex auto-review
 * fallback metadata used by the Rust parser and the selection rules the Rust
 * runtime loader applies to live models.dev responses. Run via
 * `just gen-models-dev-pricing` (see the sibling `default.nix`).
 */
import { generateCatalog } from './packages/core/src/generate.ts';
import {
	buildModelsDevCatalogIndex,
	formatDuplicateModelsDevPricingKeyWarning,
	isPriceableModelsDevCost,
	isTierVariantOfAuthoredModel,
	isTokenPricedModel,
	type ModelsDevCostTier,
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
	release_date?: string;
	modalities?: { input?: readonly string[]; output?: readonly string[] };
};
type Provider = { id?: string; models?: Record<string, Model> };
type EmbeddedModel = {
	cost: Cost;
	limit?: { context: number };
	/** Only a request naming this id exactly may use it; see `compact.ts`. */
	exactOnly?: true;
};
type CodexAutoReviewFallback = {
	releasedOn: string;
	model: string;
};

const { models, providers } = (await generateCatalog('.')) as {
	models: Record<string, ModelMetadata>;
	providers: Record<string, Provider>;
};

const catalogIndex = buildModelsDevCatalogIndex(models, providers);

const selected: Record<string, { candidate: ModelsDevPricingCandidate; entry: EmbeddedModel }> = {};
for (const [providerId, provider] of sortedEntries(providers)) {
	// A catalog is filed under its own id, but it declares one too, and the Rust
	// runtime loader resolves `provider.id ?? providerId` before it ranks. Resolve
	// it once here as well, so a catalog filed under an alias is not an owner
	// online and a reseller in the snapshot.
	const sourceProviderId = provider.id ?? providerId;
	for (const [modelId, model] of sortedEntries(provider.models ?? {})) {
		const trust = modelsDevProviderTrust({
			providerId: sourceProviderId,
			sourceModelId: modelId,
			index: catalogIndex,
		});
		const cost = model.cost ?? {};
		// Skip entries without the base prices the runtime loader requires, and
		// models whose rates are per asset rather than per text token.
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
		// The long-context band keeps the upstream `cost.tiers` shape, so the Rust
		// loader reads the snapshot and a live `api.json` response with one parser.
		const longContextTier = selectLongContextTier(cost.tiers);
		const entry: EmbeddedModel = {
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
		// A tier is the right rate only for a request naming it. Left reachable by
		// the fuzzy lookup it outranks the base model, which prefers the longest
		// matching key. An id with no version in it is the mirror image: it is
		// short enough to be a substring of unrelated ids.
		if (
			isTierVariantOfAuthoredModel(modelId, catalogIndex, { includeAuthorPricedModes: true }) ||
			isUnversionedModelId(pricingKey)
		) {
			entry.exactOnly = true;
		}
		const candidate: ModelsDevPricingCandidate = {
			sourceProviderId,
			sourceModelId: modelId,
			trust,
			hasContextLimit: entry.limit != null,
			hasExplicitCacheRead: cost.cache_read != null,
			hasExplicitCacheWrite: cost.cache_write != null,
		};
		const existing = selected[pricingKey];
		if (existing != null) {
			const replacement = shouldReplaceModelsDevPricingCandidate(existing.candidate, candidate);
			// Duplicates across trust tiers are the normal case and are resolved by
			// tier, so only report ambiguity the tiers cannot settle.
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
		selected[pricingKey] = { candidate, entry };
	}
}

const out = Object.fromEntries(
	Object.entries(selected).map(([pricingKey, { entry }]) => [pricingKey, entry]),
);

// Stable key ordering keeps the committed snapshot's diffs minimal across regenerations.
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

const codexFallbacksOutfile = process.env.CODEX_AUTO_REVIEW_FALLBACKS_OUTFILE;
if (codexFallbacksOutfile != null && codexFallbacksOutfile.length > 0) {
	await Bun.write(
		codexFallbacksOutfile,
		`${JSON.stringify(generateCodexAutoReviewFallbacks(models), null, 2)}\n`,
	);
}

function generateCodexAutoReviewFallbacks(
	models: Record<string, ModelMetadata>,
): CodexAutoReviewFallback[] {
	const entries = Object.entries(models).filter(([modelId, model]) =>
		isCodexAutoReviewFallbackCandidate(modelId, model),
	);
	const codexDecimalVersions = new Set(
		entries
			.map(([modelId]) => codexDecimalVersion(openAiModelName(modelId)))
			.filter((version): version is string => version != null),
	);

	return entries
		.filter(([modelId, model]) => {
			const version = baseDecimalVersion(openAiModelName(modelId));
			if (version == null || !codexDecimalVersions.has(version)) {
				return true;
			}
			// Drop the base entry only when a `-codex` variant shipped on the same
			// date. If the codex variant ships later, keep the base so events in
			// the gap still resolve to the most recent model available then.
			return !entries.some(
				([candidateId, candidateModel]) =>
					codexDecimalVersion(openAiModelName(candidateId)) === version &&
					candidateModel.release_date === model.release_date,
			);
		})
		.map(([modelId, model]) => ({
			releasedOn: model.release_date!,
			model: openAiModelName(model.id ?? modelId),
		}))
		.sort((left, right) => right.releasedOn.localeCompare(left.releasedOn));
}

function isCodexAutoReviewFallbackCandidate(modelId: string, model: ModelMetadata): boolean {
	if (model.release_date == null || !/^\d{4}-\d{2}-\d{2}$/.test(model.release_date)) {
		return false;
	}
	const modelName = openAiModelName(modelId);
	return (
		modelName === 'gpt-5' ||
		modelName === 'gpt-5-codex' ||
		/^gpt-5\.\d+$/.test(modelName) ||
		/^gpt-5\.\d+-codex$/.test(modelName)
	);
}

function baseDecimalVersion(modelId: string): string | undefined {
	return /^gpt-5\.\d+$/.test(modelId) ? modelId : undefined;
}

function codexDecimalVersion(modelId: string): string | undefined {
	const match = /^(gpt-5\.\d+)-codex$/.exec(modelId);
	return match?.[1];
}

function openAiModelName(modelId: string): string {
	return modelId.startsWith('openai/') ? modelId.slice('openai/'.length) : modelId;
}
