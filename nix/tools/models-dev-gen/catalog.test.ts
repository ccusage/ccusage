import assert from 'node:assert/strict';
import { it } from 'node:test';
import { restoreCatalogIdentifiers } from './catalog.ts';

void it('restores filesystem escapes before model IDs enter pricing selection', () => {
	assert.deepEqual(
		restoreCatalogIdentifiers({
			'qwen~nix~case~hack~1/qwen3.7-max': {
				id: 'qwen~nix~case~hack~1/qwen3.7-max',
				cost: { input: 2.8, output: 7.5 },
			},
			'Qwen/Qwen3.7-Max': { id: 'Qwen/Qwen3.7-Max' },
		}),
		{
			'qwen/qwen3.7-max': {
				id: 'qwen/qwen3.7-max',
				cost: { input: 2.8, output: 7.5 },
			},
			'Qwen/Qwen3.7-Max': { id: 'Qwen/Qwen3.7-Max' },
		},
	);
});

void it('preserves identifiers without filesystem escapes and optional IDs', () => {
	assert.deepEqual(restoreCatalogIdentifiers({ 'author/model': { cost: 3 } }), {
		'author/model': { cost: 3 },
	});
});

void it('rejects identifiers that collide after restoring filesystem escapes', () => {
	assert.throws(
		() =>
			restoreCatalogIdentifiers({
				'author/model': { id: 'author/model' },
				'author~nix~case~hack~1/model': { id: 'author/model' },
			}),
		/Duplicate catalog identifier/,
	);
});
