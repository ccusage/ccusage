/**
 * Generate the committed models.dev pricing snapshot.
 *
 * models.dev ships per-model TOML sources rather than a prebuilt catalog, so we
 * reuse its own `generateCatalog` routine (the same code that backs
 * https://models.dev/api.json) and then compact the result down to the
 * Anthropic- and Kimi/Moonshot-relevant models and the few pricing fields
 * ccusage consumes. The embedded output is a flat map keyed by runtime model
 * id. The output is committed to the repository and embedded at build time, so
 * every platform ships the identical, pinned data without any build-time
 * network access. The same pinned catalog also generates the Codex auto-review
 * fallback metadata used by the Rust parser. Run via `just gen-models-dev-pricing`
 * (see `nix/models-dev-pricing.nix`).
 */
import { generateCatalog } from './packages/core/src/generate.ts';
import {
	formatDuplicateModelsDevPricingKeyWarning,
	type ModelsDevPricingCandidate,
	selectModelsDevPricingKey,
	shouldReplaceModelsDevPricingCandidate,
} from './models-dev-compact.ts';

/**
 * Model ids/keys we keep in the committed snapshot.
 * Claude remains first-class; Kimi/Moonshot is included so offline pricing can
 * cover models LiteLLM has not published yet (for example kimi-k3).
 */
const KEEP = /claude|anthropic|kimi|moonshot/i;

type Cost = {
	input?: number | null;
	output?: number | null;
	cache_read?: number | null;
	cache_write?: number | null;
};
type Model = { id?: string; cost?: Cost; limit?: { context?: number | null } };
type ModelMetadata = {
	id?: string;
	release_date?: string;
};
type Provider = { id?: string; models?: Record<string, Model> };
type EmbeddedModel = {
	cost: Cost;
	limit?: { context: number };
};
type CodexAutoReviewFallback = {
	releasedOn: string;
	model: string;
};

const { models, providers } = (await generateCatalog('.')) as {
	models: Record<string, ModelMetadata>;
	providers: Record<string, Provider>;
};

const selected: Record<string, { candidate: ModelsDevPricingCandidate; entry: EmbeddedModel }> = {};
for (const [providerId, provider] of sortedEntries(providers)) {
	for (const [modelId, model] of sortedEntries(provider.models ?? {})) {
		// models.dev also exposes the canonical id under `id`; match either so
		// provider-prefixed aliases (e.g. us.anthropic.*) are kept too.
		if (!(KEEP.test(modelId) || KEEP.test(model.id ?? ''))) {
			continue;
		}
		const cost = model.cost ?? {};
		// Skip entries without the base prices the runtime loader requires.
		// Also drop all-zero placeholder costs (for example kimi-for-coding/k3).
		if (cost.input == null || cost.output == null) {
			continue;
		}
		if (cost.input === 0 && cost.output === 0) {
			continue;
		}
		const pricingKey = selectModelsDevPricingKey(modelId, model.id);
		const entry: EmbeddedModel = {
			cost: {
				input: cost.input,
				output: cost.output,
				...(cost.cache_read != null ? { cache_read: cost.cache_read } : {}),
				...(cost.cache_write != null ? { cache_write: cost.cache_write } : {}),
			},
		};
		if (model.limit?.context != null) {
			entry.limit = { context: model.limit.context };
		}
		const candidate: ModelsDevPricingCandidate = {
			sourceProviderId: provider.id ?? providerId,
			sourceModelId: modelId,
			hasContextLimit: entry.limit != null,
			hasExplicitCacheRead: cost.cache_read != null,
			hasExplicitCacheWrite: cost.cache_write != null,
		};
		const existing = selected[pricingKey];
		if (existing != null) {
			const replacement = shouldReplaceModelsDevPricingCandidate(existing.candidate, candidate);
			console.warn(
				formatDuplicateModelsDevPricingKeyWarning({
					pricingKey,
					sourceModelId: replacement ? existing.candidate.sourceModelId : candidate.sourceModelId,
				}),
			);
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
