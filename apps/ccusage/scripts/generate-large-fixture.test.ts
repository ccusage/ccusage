import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { describe, it } from 'node:test';

const generator = path.join(import.meta.dirname, 'generate-large-fixture.ts');

void describe('generate-large-fixture', () => {
	void it('refuses a non-empty Codex directory before writing the Claude fixture', async () => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		try {
			const claudeDir = path.join(root, 'claude');
			const codexDir = path.join(root, 'codex');
			await mkdir(codexDir);
			await writeFile(path.join(codexDir, 'existing.jsonl'), '');

			const result = spawnSync(
				process.execPath,
				[
					generator,
					'--output-dir',
					claudeDir,
					'--codex-output-dir',
					codexDir,
					'--size-mib',
					'1',
					'--codex-size-mib',
					'1',
				],
				{ encoding: 'utf8' },
			);

			assert.notEqual(result.status, 0);
			assert.match(result.stderr, /--codex-output-dir must be absent or empty/);
			assert.deepEqual(await readdir(claudeDir), []);
		} finally {
			await rm(root, { recursive: true, force: true });
		}
	});
});
