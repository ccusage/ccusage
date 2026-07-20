export function selectModelsDevPricingKey(modelId: string, catalogId: string | undefined): string {
	return catalogId != null && catalogId.length > 0 ? catalogId : modelId;
}

export type ModelsDevPricingCandidate = {
	sourceProviderId: string;
	sourceModelId: string;
	hasContextLimit: boolean;
	hasExplicitCacheRead: boolean;
	hasExplicitCacheWrite: boolean;
};

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
		compareNumber(candidateProviderPriority(left), candidateProviderPriority(right)) ||
		compareBoolean(left.hasExplicitCacheRead, right.hasExplicitCacheRead) ||
		compareBoolean(left.hasExplicitCacheWrite, right.hasExplicitCacheWrite) ||
		compareBoolean(left.hasContextLimit, right.hasContextLimit) ||
		compareStringPreferSmaller(left.sourceProviderId, right.sourceProviderId) ||
		compareStringPreferSmaller(left.sourceModelId, right.sourceModelId)
	);
}

function candidateProviderPriority(candidate: ModelsDevPricingCandidate): number {
	// Prefer first-party provider catalogs when several resellers publish the
	// same pricing key (for example moonshotai vs venice for kimi-k3).
	if (
		candidate.sourceProviderId === 'anthropic' ||
		candidate.sourceProviderId === 'moonshotai' ||
		candidate.sourceProviderId === 'moonshot'
	) {
		return 2;
	}
	if (
		candidate.sourceModelId.includes('anthropic') ||
		candidate.sourceModelId.includes('moonshot') ||
		candidate.sourceModelId.includes('kimi')
	) {
		return 1;
	}
	return 0;
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
