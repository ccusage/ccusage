import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { describe, it } from 'node:test';

const generator = path.join(import.meta.dirname, 'generate-large-fixture.ts');

void describe('generate-large-fixture', () => {
	void it('matches the dominant Claude usage order and body statistics while exercising fallback shapes', async (t) => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		t.after(async () => rm(root, { recursive: true, force: true }));

		const result = spawnSync(
			process.execPath,
			[generator, '--output-dir', root, '--size-mib', '16'],
			{ encoding: 'utf8' },
		);
		assert.equal(result.status, 0, result.stderr);

		const modernOrder = [
			'parentUuid',
			'isSidechain',
			'message',
			'requestId',
			'type',
			'uuid',
			'timestamp',
			'userType',
			'entrypoint',
			'cwd',
			'sessionId',
			'version',
			'gitBranch',
		];
		const files = await readdir(root, { recursive: true });
		let directUsage = 0;
		let modernUsage = 0;
		let metadataFirstUsage = 0;
		let wrappedUsage = 0;
		let userRows = 0;
		let lineBytes = 0;
		let quotes = 0;
		let bodyBytes = 0;
		let escapes = 0;
		let nonAsciiBytes = 0;
		for (const file of files.filter((name) => name.endsWith('.jsonl'))) {
			const lines = (await readFile(path.join(root, file), 'utf8')).trimEnd().split('\n');
			for (const line of lines) {
				const row = JSON.parse(line) as {
					type: string;
					message?: { usage?: unknown; content?: unknown };
					data?: { message?: { message?: { usage?: unknown } } };
				};
				userRows += Number(row.type === 'user');
				wrappedUsage += Number(row.data?.message?.message?.usage !== undefined);
				if (row.message?.usage === undefined) {
					continue;
				}
				directUsage += 1;
				const keys = Object.keys(row).filter((key) => modernOrder.includes(key));
				modernUsage += Number(keys.join() === modernOrder.join());
				const messageIndex = keys.indexOf('message');
				metadataFirstUsage += Number(
					['type', 'timestamp', 'sessionId', 'version'].every(
						(key) => keys.indexOf(key) < messageIndex,
					),
				);
				lineBytes += Buffer.byteLength(line);
				quotes += line.match(/"/gu)?.length ?? 0;
				const body = JSON.stringify(row.message.content);
				bodyBytes += Buffer.byteLength(body);
				escapes += body.match(/\\./gu)?.length ?? 0;
				nonAsciiBytes += Buffer.byteLength(body.replace(/\p{ASCII}/gu, ''));
			}
		}

		const modernShare = modernUsage / (directUsage + wrappedUsage);
		assert.ok(modernShare >= 0.97 && modernShare <= 0.995, `Modern usage share: ${modernShare}`);
		assert.ok(metadataFirstUsage > 0);
		assert.equal(modernUsage + metadataFirstUsage, directUsage);
		assert.ok(wrappedUsage > 0);
		assert.ok(userRows > 0);
		const meanLineBytes = lineBytes / directUsage;
		assert.ok(meanLineBytes >= 2800 && meanLineBytes <= 3200, `Mean line bytes: ${meanLineBytes}`);
		const meanQuotes = quotes / directUsage;
		assert.ok(meanQuotes >= 177 && meanQuotes <= 206, `Mean quotes: ${meanQuotes}`);
		const escapesPerKiB = (escapes * 1024) / bodyBytes;
		assert.ok(escapesPerKiB >= 6 && escapesPerKiB <= 10, `Escapes per KiB: ${escapesPerKiB}`);
		const nonAsciiShare = nonAsciiBytes / bodyBytes;
		assert.ok(nonAsciiShare >= 0.02 && nonAsciiShare <= 0.04, `Non-ASCII share: ${nonAsciiShare}`);
	});

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
