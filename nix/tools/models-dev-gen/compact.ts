/**
 * Selection rules shared by the models.dev snapshot generator and its tests.
 *
 * models.dev publishes one catalog per provider, so the same model appears
 * dozens of times: once from whoever authored it, once per cloud platform that
 * hosts it, and once per reseller that marks it up or discounts it. Only the
 * authoring catalog is guaranteed to carry list pricing, so picking the wrong
 * duplicate silently bills users at a marketplace rate.
 *
 * Every id the catalog publishes is embedded, and the rules here only decide
 * *which* catalog's rates a given id gets. Pruning ids to keep the snapshot
 * small was tried and abandoned: `PricingMap::find` falls back to substring
 * matching that prefers the longest key, so removing an id makes the lookup
 * guess between a base model and a premium tier. Keeping every id means each one
 * resolves exactly, and the guessing is confined to ids the catalog has never
 * heard of - which is where it was before any of this.
 */

/**
 * Trust tiers used to break ties between duplicate pricing keys. Higher wins.
 */
export const MODELS_DEV_PROVIDER_TRUST = {
	/** The catalog of whoever authored the model. Publishes list pricing. */
	owner: 3,
	/**
	 * Cloud platforms that resell at list price plus a documented regional
	 * premium. They are the only source for the platform-specific model ids
	 * agents record, such as the Bedrock `us.anthropic.*` inference profiles.
	 */
	platform: 2,
	/**
	 * Everyone else. Their prices are their own - promotions, markups, and
	 * third-party GPU hosts undercutting the author - so they are a last resort
	 * for models no trusted catalog publishes any more, such as retired Claude 3
	 * releases that only resellers still list.
	 */
	reseller: 1,
} as const satisfies Record<string, number>;

/**
 * Provider ids that host models at list price plus a published regional
 * premium, rather than setting their own.
 */
const PLATFORM_PROVIDER_IDS = [
	'amazon-bedrock',
	'azure',
	'azure-cognitive-services',
	'google-vertex',
	'google-vertex-anthropic',
] as const satisfies readonly string[];

/**
 * Provider ids that serve their own models under a different name than the
 * `models/<author>/` directory they are authored in, so the directory scan
 * alone cannot recognize them as first-party.
 */
const FIRST_PARTY_PROVIDER_ID_ALIASES = [
	// Z.ai authors GLM under `models/zhipuai/` but serves it as `zai`.
	'zai',
] as const satisfies readonly string[];

/** One above-base rate band, as `cost.tiers[]` publishes it. */
export type ModelsDevCostTier = {
	input?: number | null;
	output?: number | null;
	cache_read?: number | null;
	cache_write?: number | null;
	tier?: { type?: string; size?: number | null };
};

export type ModelsDevModalities = {
	input?: readonly string[];
	output?: readonly string[];
};

/**
 * Index of the canonical `models/<author>/<id>.toml` catalog, used to decide
 * which provider authored a model without hardcoding a provider list.
 */
export type ModelsDevCatalogIndex = {
	/** `<author>/<id>` keys, exactly as `generateModels` returns them. */
	authoredKeys: ReadonlySet<string>;
	/** Directory names under `models/`, i.e. the set of authoring providers. */
	authorProviderIds: ReadonlySet<string>;
	/** The same keys with the author prefix stripped. */
	authoredModelIds: ReadonlySet<string>;
	/** `authoredModelIds` normalized for prefix comparison. */
	normalizedAuthoredModelIds: readonly string[];
	/** Normalized authored id -> the modes that id publishes its own rates for. */
	authoredModes: ReadonlyMap<string, ReadonlySet<string>>;
	/** Authored modalities keyed by the normalized bare model id. */
	authoredModalities: ReadonlyMap<string, ModelsDevModalities>;
};

export type ModelsDevPricingCandidate = {
	sourceProviderId: string;
	sourceModelId: string;
	trust: number;
	hasLongContextTier: boolean;
	hasContextLimit: boolean;
	hasExplicitCacheRead: boolean;
	hasExplicitCacheWrite: boolean;
};

/**
 * Build the authorship index from the canonical catalog.
 *
 * `providerCatalogs` is read only for the modes an authoring provider prices
 * itself, because those live in `providers/<author>/models/<id>.toml` rather
 * than in the `models/` metadata.
 *
 * @param authoredModels - `<author>/<id>` keyed models from `generateCatalog().models`.
 * @param providerCatalogs - provider id keyed catalogs from `generateCatalog().providers`.
 * @example
 * const index = buildModelsDevCatalogIndex({ 'anthropic/claude-opus-5': {} });
 * index.authorProviderIds.has('anthropic'); // true
 */
export function buildModelsDevCatalogIndex(
	authoredModels: Readonly<Record<string, { modalities?: ModelsDevModalities }>>,
	providerCatalogs: Readonly<
		Record<
			string,
			{
				models?: Readonly<
					Record<string, { experimental?: { modes?: Readonly<Record<string, unknown>> } }>
				>;
			}
		>
	> = {},
): ModelsDevCatalogIndex {
	const authorProviderIds = new Set<string>();
	const authoredModelIds = new Set<string>();
	const authoredModalities = new Map<string, ModelsDevModalities>();
	for (const [key, model] of Object.entries(authoredModels)) {
		const separator = key.indexOf('/');
		if (separator <= 0) {
			continue;
		}
		const modelId = key.slice(separator + 1);
		authorProviderIds.add(key.slice(0, separator));
		authoredModelIds.add(modelId);
		if (model.modalities != null) {
			authoredModalities.set(normalizeModelId(modelId), model.modalities);
		}
	}
	const authoredModes = new Map<string, Set<string>>();
	for (const [providerId, catalog] of Object.entries(providerCatalogs)) {
		if (
			!authorProviderIds.has(providerId) &&
			!(FIRST_PARTY_PROVIDER_ID_ALIASES as readonly string[]).includes(providerId)
		) {
			continue;
		}
		for (const [modelId, model] of Object.entries(catalog.models ?? {})) {
			const modes = Object.keys(model.experimental?.modes ?? {}).map(normalizeModelId);
			if (modes.length === 0) {
				continue;
			}
			const normalized = normalizeModelId(modelId);
			const existing = authoredModes.get(normalized) ?? new Set<string>();
			for (const mode of modes) {
				existing.add(mode);
			}
			authoredModes.set(normalized, existing);
		}
	}
	return {
		authoredKeys: new Set(Object.keys(authoredModels)),
		authorProviderIds,
		authoredModelIds,
		normalizedAuthoredModelIds: [...authoredModelIds].map(normalizeModelId),
		authoredModes,
		authoredModalities,
	};
}

/** Model ids are spelled with either dots or dashes for the same version. */
export function normalizeModelId(modelId: string): string {
	return modelId.toLowerCase().replace(/[.@]/g, '-');
}

/**
 * Trust tier for one provider catalog entry.
 *
 * @example
 * modelsDevProviderTrust({ providerId: 'openrouter', sourceModelId: 'kimi-k3', index });
 * // MODELS_DEV_PROVIDER_TRUST.reseller
 */
export function modelsDevProviderTrust({
	providerId,
	sourceModelId,
	index,
}: {
	providerId: string;
	sourceModelId: string;
	index: ModelsDevCatalogIndex;
}): number {
	// An exact `<provider>/<model>` hit in the authored catalog is the strongest
	// signal. The namespace check covers models a provider serves without a
	// canonical metadata file of their own, such as `openai/gpt-5.6`.
	if (
		index.authoredKeys.has(`${providerId}/${sourceModelId}`) ||
		index.authorProviderIds.has(providerId) ||
		(FIRST_PARTY_PROVIDER_ID_ALIASES as readonly string[]).includes(providerId)
	) {
		return MODELS_DEV_PROVIDER_TRUST.owner;
	}
	if ((PLATFORM_PROVIDER_IDS as readonly string[]).includes(providerId)) {
		return MODELS_DEV_PROVIDER_TRUST.platform;
	}
	return MODELS_DEV_PROVIDER_TRUST.reseller;
}

/**
 * Whether an id identifies no particular model, so it must not answer a lookup
 * for a longer id.
 *
 * A model id nearly always carries a version - `kimi-k2.6`, `gpt-5.4`,
 * `claude-opus-5`. An id with no digit at all is a family or a routing label:
 * models.dev carries one called `auto`, and as a fuzzy candidate it answered
 * `codex-auto-review`, a Codex label that must resolve through the adapter's
 * date mapping instead of being priced directly.
 */
export function isUnversionedModelId(sourceModelId: string): boolean {
	return !/\d/.test(sourceModelId);
}

/**
 * Whether an id names a separately priced tier of a model the snapshot also
 * carries under its base id - `kimi-k2.6-nitro`, `glm-5.2-flex`,
 * `kimi-k2.7-code-highspeed`, `claude-opus-5-fast`.
 *
 * Such an entry is the right rate only for a request naming it, so the snapshot
 * marks it exact-only. Left reachable by the fuzzy lookup it answers the base
 * model instead, because that lookup prefers the longest matching key: a request
 * for `kimi-k2-7-code` was billed at the highspeed tier, double the list rate.
 *
 * Only bare ids qualify. An id carrying a provider path is that gateway's
 * addressing of a model, and it has to stay fuzzy-reachable so the gateway's own
 * tier spellings still resolve to a tier.
 *
 * @param includeAuthorPricedModes - count tiers the author prices itself. The
 * shadowing hazard does not care who set the rate, but the embedding decision
 * once did, so the two callers want different answers.
 */
export function isTierVariantOfAuthoredModel(
	sourceModelId: string,
	index: ModelsDevCatalogIndex,
	{ includeAuthorPricedModes = false }: { includeAuthorPricedModes?: boolean } = {},
): boolean {
	if (sourceModelId.includes('/')) {
		return false;
	}
	const normalized = normalizeModelId(sourceModelId);
	return index.normalizedAuthoredModelIds.some((authored) => {
		if (!normalized.startsWith(`${authored}-`)) {
			return false;
		}
		if (includeAuthorPricedModes) {
			return true;
		}
		const tier = normalized.slice(authored.length + 1);
		return !index.authoredModes.get(authored)?.has(tier);
	});
}

/**
 * The selection rules the Rust runtime loader needs, for the same decisions this
 * module makes at generation time. The runtime sees only the live `api.json`, so
 * it can neither scan the authored catalog for authorship nor read authored
 * modalities, and both have to be carried in.
 */
export function modelsDevCatalogRulesArtifact(index: ModelsDevCatalogIndex): {
	owners: string[];
	platforms: string[];
	authoredModelIds: string[];
	authoredModes: Record<string, string[]>;
	assetPricedModelIds: string[];
} {
	const assetPricedModelIds = [
		...new Set(
			[...index.authoredModelIds]
				.filter(
					(sourceModelId) => !isTokenPricedModel({ sourceModelId, modalities: undefined, index }),
				)
				.map(normalizeModelId),
		),
	].sort();
	return {
		owners: [...index.authorProviderIds, ...FIRST_PARTY_PROVIDER_ID_ALIASES].sort(),
		platforms: [...PLATFORM_PROVIDER_IDS].sort(),
		// The runtime needs both lists: an authored id absent from the asset list
		// is authored as token-priced, which settles it without consulting
		// whichever catalog the live response happens to serve it from. Emitted
		// normalized, and looked up normalized, so a catalog spelling the model
		// with different separators or case cannot dodge the verdict.
		authoredModelIds: [...new Set([...index.authoredModelIds].map(normalizeModelId))].sort(),
		// The tiers an author prices itself, so the runtime can tell a reseller-only
		// tier worth carrying from a reseller's markup on a published rate, which is
		// the rest of what `isEmbeddableModelsDevCandidate` decides.
		authoredModes: Object.fromEntries(
			[...index.authoredModes]
				.map(([modelId, modes]): [string, string[]] => [modelId, [...modes].sort()])
				.sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0)),
		),
		assetPricedModelIds,
	};
}

/**
 * Whether a models.dev cost block can price tokens at all.
 *
 * Flat-fee subscription catalogs such as `kimi-for-coding` publish all-zero
 * costs, which would otherwise embed as a free model.
 */
export function isPriceableModelsDevCost<
	Value extends { input?: number | null; output?: number | null },
>(cost: Value): cost is Value & { input: number; output: number } {
	const { input, output } = cost;
	if (input == null || output == null) {
		return false;
	}
	return input !== 0 || output !== 0;
}

/**
 * Whether a model bills per text token, so the embedded `input` and `output`
 * rates mean what the runtime assumes when it multiplies them by token counts.
 *
 * Two signals, both read off the modalities:
 *
 * - Output must be text only. An image or audio output rate is per asset, so
 *   `gemini-2.5-flash-image`'s 30 USD output rate is per image, not per Mtok.
 * - Input must accept text. A model that accepts no text is a transcription or
 *   vision-only endpoint billed by duration - `whisper-large-v3` accepts audio
 *   alone and prices per second. Accepting audio, video, image or PDF *as well
 *   as* text is normal for chat models and is tokenised, so it stays eligible.
 *
 * The authored catalog decides, not the catalog being read. Reseller catalogs
 * describe the same model less carefully, and one claiming text-only output for
 * an image model would otherwise smuggle a per-image rate into the snapshot.
 *
 * @example
 * // authored as input: ["audio"], so excluded whichever catalog serves it
 * isTokenPricedModel({ sourceModelId: 'whisper-large-v3', modalities: { output: ['text'] }, index });
 * // false
 */
export function isTokenPricedModel({
	sourceModelId,
	modalities,
	index,
}: {
	sourceModelId: string;
	modalities: ModelsDevModalities | undefined;
	index: ModelsDevCatalogIndex;
}): boolean {
	// Keyed by the normalized id, so a catalog spelling the model with different
	// separators or case still gets the authored verdict.
	const normalized = normalizeModelId(sourceModelId);
	const authored = index.authoredModalities.get(normalized);
	if (authored != null) {
		return billsPerTextToken(authored);
	}
	// An authored model that publishes no modalities is token-priced: the Rust
	// loader keeps every authored id it has no asset verdict for, so skipping it
	// here would make the snapshot and a live refresh disagree.
	if (index.normalizedAuthoredModelIds.includes(normalized)) {
		return true;
	}
	return billsPerTextToken(modalities);
}

function billsPerTextToken(modalities: ModelsDevModalities | undefined): boolean {
	const output = modalities?.output ?? ['text'];
	if (output.length !== 1 || output[0] !== 'text') {
		return false;
	}
	const input = modalities?.input ?? ['text'];
	return input.includes('text');
}

export function selectModelsDevPricingKey(modelId: string, catalogId: string | undefined): string {
	return catalogId != null && catalogId.length > 0 ? catalogId : modelId;
}

export function shouldReplaceModelsDevPricingCandidate(
	existing: ModelsDevPricingCandidate,
	candidate: ModelsDevPricingCandidate,
): boolean {
	return compareModelsDevPricingCandidates(candidate, existing) > 0;
}

export function formatDuplicateModelsDevPricingKeyWarning({
	pricingKey,
	sourceModelId,
}: {
	pricingKey: string;
	sourceModelId: string;
}): string {
	return `models.dev pricing key "${pricingKey}" already exists; skipping duplicate source model "${sourceModelId}".`;
}

function compareModelsDevPricingCandidates(
	left: ModelsDevPricingCandidate,
	right: ModelsDevPricingCandidate,
): number {
	return (
		compareNumber(left.trust, right.trust) ||
		// A long-context band is the rate data hardest to come by, so within a
		// trust tier the candidate carrying one wins.
		compareBoolean(left.hasLongContextTier, right.hasLongContextTier) ||
		compareBoolean(left.hasExplicitCacheRead, right.hasExplicitCacheRead) ||
		compareBoolean(left.hasExplicitCacheWrite, right.hasExplicitCacheWrite) ||
		compareBoolean(left.hasContextLimit, right.hasContextLimit) ||
		compareStringPreferSmaller(left.sourceProviderId, right.sourceProviderId) ||
		compareStringPreferSmaller(left.sourceModelId, right.sourceModelId)
	);
}

function compareNumber(left: number, right: number): number {
	return left === right ? 0 : left > right ? 1 : -1;
}

function compareBoolean(left: boolean, right: boolean): number {
	return compareNumber(left ? 1 : 0, right ? 1 : 0);
}

function compareStringPreferSmaller(left: string, right: string): number {
	return left === right ? 0 : left < right ? 1 : -1;
}

/**
 * The long-context band to embed for a model, or `undefined` when it has none.
 *
 * `Pricing` holds a single above-base band, so the lowest context threshold is
 * the one kept: it is the band a request crosses first, and the 15 models that
 * publish a second one would otherwise contribute nothing at all. Bands keyed by
 * anything other than context are skipped, because the runtime compares them
 * against an input-token count.
 *
 * @example
 * selectLongContextTier([{ input: 4, output: 12, tier: { type: 'context', size: 200000 } }]);
 * // the same band back, ready to embed
 */
export function selectLongContextTier(
	tiers: readonly ModelsDevCostTier[] | undefined,
): ModelsDevCostTier | undefined {
	const contextTiers = (tiers ?? []).filter(
		(tier) => tier.tier?.type === 'context' && (tier.tier?.size ?? 0) > 0,
	);
	if (contextTiers.length === 0) {
		return undefined;
	}
	return contextTiers.reduce((lowest, tier) =>
		(tier.tier?.size ?? 0) < (lowest.tier?.size ?? 0) ? tier : lowest,
	);
}
