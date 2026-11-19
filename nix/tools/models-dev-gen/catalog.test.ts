import assert from 'node:assert/strict';
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { it } from 'node:test';
import { rejectEscapedFileNames, restoreCatalogIdentifiers } from './catalog.ts';

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

void it('preserves __proto__ as an own catalog identifier', () => {
	const restored = restoreCatalogIdentifiers({
		['__proto__']: { cost: 3 },
	});
	assert.deepEqual(Object.entries(restored), [['__proto__', { cost: 3 }]]);
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

function withSourceTree(files: readonly string[], test: (root: string) => void): void {
	const root = mkdtempSync(path.join(tmpdir(), 'models-dev-gen-'));
	try {
		for (const file of files) {
			mkdirSync(path.dirname(path.join(root, file)), { recursive: true });
			writeFileSync(path.join(root, file), '');
		}
		test(root);
	} finally {
		rmSync(root, { recursive: true, force: true });
	}
}

void it('accepts escaped directories, which identifier restoration repairs', () => {
	withSourceTree(
		['models/author/model.toml', 'providers/nearai/models/qwen~nix~case~hack~1/qwen3.7-max.toml'],
		(root) => {
			rejectEscapedFileNames([path.join(root, 'models'), path.join(root, 'providers')]);
		},
	);
});

void it('rejects escaped file names, which the catalog glob would silently skip', () => {
	withSourceTree(
		[
			'models/author/model.toml',
			'providers/acme/models/model.toml',
			'providers/acme/models/model.toml~nix~case~hack~1',
		],
		(root) => {
			assert.throws(
				() => rejectEscapedFileNames([path.join(root, 'models'), path.join(root, 'providers')]),
				/providers\/acme\/models\/model\.toml~nix~case~hack~1/,
			);
		},
	);
});
