export const MODELS_DEV_PROVIDER_TRUST = {
	owner: 3,
	platform: 2,
	reseller: 1,
} as const satisfies Record<string, number>;

const PLATFORM_PROVIDER_IDS = [
	'amazon-bedrock',
	'azure',
	'azure-cognitive-services',
	'google-vertex',
	'google-vertex-anthropic',
] as const satisfies readonly string[];

const FIRST_PARTY_PROVIDER_ID_ALIASES = ['zai'] as const satisfies readonly string[];

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

export type ModelsDevCatalogIndex = {
	authoredKeys: ReadonlySet<string>;
	authorProviderIds: ReadonlySet<string>;
	authoredModelIds: ReadonlySet<string>;
	normalizedAuthoredModelIds: readonly string[];
	authoredModes: ReadonlyMap<string, ReadonlySet<string>>;
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

export function normalizeModelId(modelId: string): string {
	return modelId.toLowerCase().replace(/[.@]/g, '-');
}

export function modelsDevProviderTrust({
	providerId,
	sourceModelId,
	index,
}: {
	providerId: string;
	sourceModelId: string;
	index: ModelsDevCatalogIndex;
}): number {
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

export function isUnversionedModelId(sourceModelId: string): boolean {
	return !/\d/.test(sourceModelId);
}

export function isTierVariantOfAuthoredModel(
	sourceModelId: string,
	index: ModelsDevCatalogIndex,
): boolean {
	if (sourceModelId.includes('/')) {
		return false;
	}
	const normalized = normalizeModelId(sourceModelId);
	return index.normalizedAuthoredModelIds.some((authored) => normalized.startsWith(`${authored}-`));
}

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
		authoredModelIds: [...new Set([...index.authoredModelIds].map(normalizeModelId))].sort(),
		authoredModes: Object.fromEntries(
			[...index.authoredModes]
				.map(([modelId, modes]): [string, string[]] => [modelId, [...modes].sort()])
				.sort(([left], [right]) => (left < right ? -1 : left > right ? 1 : 0)),
		),
		assetPricedModelIds,
	};
}

export function isPriceableModelsDevCost<
	Value extends { input?: number | null; output?: number | null },
>(cost: Value): cost is Value & { input: number; output: number } {
	const { input, output } = cost;
	if (input == null || output == null) {
		return false;
	}
	return input !== 0 || output !== 0;
}

export function isTokenPricedModel({
	sourceModelId,
	modalities,
	index,
}: {
	sourceModelId: string;
	modalities: ModelsDevModalities | undefined;
	index: ModelsDevCatalogIndex;
}): boolean {
	const normalized = normalizeModelId(sourceModelId);
	const authored = index.authoredModalities.get(normalized);
	if (authored != null) {
		return billsPerTextToken(authored);
	}
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
