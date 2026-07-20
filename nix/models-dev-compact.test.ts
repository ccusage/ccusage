import assert from 'node:assert/strict';
import { it } from 'node:test';
import {
	formatDuplicateModelsDevPricingKeyWarning,
	shouldReplaceModelsDevPricingCandidate,
	selectModelsDevPricingKey,
} from './models-dev-compact.ts';

void it('falls back to the source model id when the catalog id is empty', () => {
	assert.equal(
		selectModelsDevPricingKey('anthropic/claude-sonnet-4', ''),
		'anthropic/claude-sonnet-4',
	);
});

void it('falls back to the source model id when the catalog id is undefined', () => {
	assert.equal(
		selectModelsDevPricingKey('anthropic/claude-sonnet-4', undefined),
		'anthropic/claude-sonnet-4',
	);
});

void it('uses the catalog id when it is non-empty', () => {
	assert.equal(
		selectModelsDevPricingKey('anthropic/claude-sonnet-4', 'catalog-id-123'),
		'catalog-id-123',
	);
});

void it('formats duplicate pricing key warnings with the skipped source id', () => {
	assert.equal(
		formatDuplicateModelsDevPricingKeyWarning({
			pricingKey: 'claude-sonnet-4',
			sourceModelId: 'anthropic/claude-sonnet-4',
		}),
		'models.dev pricing key "claude-sonnet-4" already exists; skipping duplicate source model "anthropic/claude-sonnet-4".',
	);
});

void it('prefers Anthropic provider pricing over duplicate aliases', () => {
	assert.equal(
		shouldReplaceModelsDevPricingCandidate(
			{
				sourceProviderId: 'github-copilot',
				sourceModelId: 'claude-sonnet-4-6',
				hasContextLimit: true,
				hasExplicitCacheRead: true,
				hasExplicitCacheWrite: true,
			},
			{
				sourceProviderId: 'anthropic',
				sourceModelId: 'claude-sonnet-4-6',
				hasContextLimit: true,
				hasExplicitCacheRead: true,
				hasExplicitCacheWrite: true,
			},
		),
		true,
	);
});

void it('prefers MoonshotAI provider pricing over reseller duplicates', () => {
	assert.equal(
		shouldReplaceModelsDevPricingCandidate(
			{
				sourceProviderId: 'venice',
				sourceModelId: 'kimi-k3',
				hasContextLimit: true,
				hasExplicitCacheRead: true,
				hasExplicitCacheWrite: false,
			},
			{
				sourceProviderId: 'moonshotai',
				sourceModelId: 'kimi-k3',
				hasContextLimit: true,
				hasExplicitCacheRead: true,
				hasExplicitCacheWrite: false,
			},
		),
		true,
	);
});

void it('uses a stable source ordering tie-break for duplicate aliases', () => {
	assert.equal(
		shouldReplaceModelsDevPricingCandidate(
			{
				sourceProviderId: 'nano-gpt',
				sourceModelId: 'claude-sonnet-4',
				hasContextLimit: true,
				hasExplicitCacheRead: true,
				hasExplicitCacheWrite: true,
			},
			{
				sourceProviderId: 'github-copilot',
				sourceModelId: 'claude-sonnet-4',
				hasContextLimit: true,
				hasExplicitCacheRead: true,
				hasExplicitCacheWrite: true,
			},
		),
		true,
	);
});
