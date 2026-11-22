import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { mkdir, mkdtemp, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { describe, it } from 'node:test';
import { zstdDecompressSync } from 'node:zlib';

const generator = path.join(import.meta.dirname, 'generate-large-fixture.ts');

type Row = Record<string, unknown>;

function isRow(value: unknown): value is Row {
	return typeof value === 'object' && value !== null;
}

function sessionMeta(row: Row | undefined): { id: unknown; forkedFromId?: unknown } {
	assert.ok(row?.type === 'session_meta' && isRow(row.payload));
	return { id: row.payload.id, forkedFromId: row.payload.forked_from_id };
}

async function readJsonlFiles(root: string): Promise<{ path: string; rows: Row[] }[]> {
	const names = (await readdir(root, { recursive: true })).filter((name) =>
		/\.jsonl(?:\.zst)?$/u.test(name),
	);
	return Promise.all(
		names.map(async (name) => {
			const file = path.join(root, name);
			const data = await readFile(file);
			const text = (name.endsWith('.zst') ? zstdDecompressSync(data) : data).toString('utf8');
			return {
				path: file,
				rows: text
					.trimEnd()
					.split('\n')
					.map((line): Row => JSON.parse(line)),
			};
		}),
	);
}

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

	void it('dates each file by its last record and forks some Codex rollouts', async (t) => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		t.after(async () => rm(root, { recursive: true, force: true }));
		const claudeDir = path.join(root, 'claude');
		const codexDir = path.join(root, 'codex');

		const result = spawnSync(
			process.execPath,
			[
				generator,
				'--output-dir',
				claudeDir,
				'--codex-output-dir',
				codexDir,
				'--size-mib',
				'8',
				'--codex-size-mib',
				'32',
			],
			{ encoding: 'utf8' },
		);
		assert.equal(result.status, 0, result.stderr);

		let datedFiles = 0;
		for (const file of await readJsonlFiles(claudeDir)) {
			const times = file.rows.flatMap((row) =>
				typeof row.timestamp === 'string' ? [Date.parse(row.timestamp)] : [],
			);
			if (times.length === 0) {
				continue;
			}
			datedFiles += 1;
			assert.equal((await stat(file.path)).mtimeMs, Math.max(...times), file.path);
		}
		assert.ok(datedFiles > 0);

		const rollouts = (await readJsonlFiles(codexDir)).map((file) => ({
			...file,
			meta: sessionMeta(file.rows[0]),
		}));
		const ids = new Set(rollouts.map((rollout) => rollout.meta.id));
		const forks = rollouts.filter((rollout) => rollout.meta.forkedFromId !== undefined);
		assert.ok(forks.length > 0);
		for (const fork of forks) {
			assert.ok(
				ids.has(fork.meta.forkedFromId ?? ''),
				`parent ${String(fork.meta.forkedFromId)} is generated`,
			);
		}
		assert.ok(
			forks.some((fork) =>
				fork.rows.some(
					(row) =>
						row.type === 'token_usage_record' &&
						isRow(row.payload) &&
						row.payload.thread_id === fork.meta.forkedFromId,
				),
			),
			'a user fork carries usage records copied from its parent',
		);
	});

	void it('ends some parent transcripts with an undated tail beside several subagent transcripts', async (t) => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		t.after(async () => rm(root, { recursive: true, force: true }));

		const result = spawnSync(
			process.execPath,
			[generator, '--output-dir', root, '--claude-shape', 'many-small-files', '--size-mib', '8'],
			{ encoding: 'utf8' },
		);
		assert.equal(result.status, 0, result.stderr);

		const files = await readJsonlFiles(root);
		const subagents = new Map<string, number>();
		for (const file of files) {
			const parts = path.relative(root, file.path).split(path.sep);
			if (parts.at(-2) === 'subagents') {
				const session = parts.slice(0, -2).join(path.sep);
				subagents.set(session, (subagents.get(session) ?? 0) + 1);
			}
		}
		const undatedTails = files.filter((file) => {
			const tail = file.rows.slice(-1);
			return (
				tail.every((row) => row.timestamp === undefined) &&
				Buffer.byteLength(JSON.stringify(tail)) > 16 * 1024 &&
				(subagents.get(path.relative(root, file.path).replace(/\.jsonl$/u, '')) ?? 0) >= 3
			);
		});
		assert.ok(undatedTails.length > 0, 'a parent with several subagents ends in an undated tail');
	});

	void it('writes requestless proxy transcripts whose lines chain through parentUuid', async (t) => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		t.after(async () => rm(root, { recursive: true, force: true }));

		const result = spawnSync(
			process.execPath,
			[generator, '--output-dir', root, '--claude-shape', 'requestless-proxy', '--size-mib', '8'],
			{ encoding: 'utf8' },
		);
		assert.equal(result.status, 0, result.stderr);

		let assistantRows = 0;
		for (const file of await readJsonlFiles(root)) {
			let previousUuid: unknown = null;
			for (const row of file.rows) {
				if (row.uuid === undefined) {
					continue;
				}
				assert.equal(row.parentUuid, previousUuid);
				previousUuid = row.uuid;
				if (row.type === 'assistant') {
					assistantRows += 1;
					assert.equal(row.requestId, undefined);
				}
			}
		}
		assert.ok(assistantRows > 0);
	});

	void it('spreads the many-small-files shape over many project directories', async (t) => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		t.after(async () => rm(root, { recursive: true, force: true }));

		const result = spawnSync(
			process.execPath,
			[generator, '--output-dir', root, '--claude-shape', 'many-small-files', '--size-mib', '8'],
			{ encoding: 'utf8' },
		);
		assert.equal(result.status, 0, result.stderr);

		const files = await readJsonlFiles(root);
		const projects = new Set(
			files.map((file) => path.relative(root, file.path).split(path.sep)[1]),
		);
		assert.ok(files.length >= 500, `Files: ${files.length}`);
		assert.ok(projects.size >= 400, `Projects: ${projects.size}`);
	});

	void it('writes an OpenCode WAL database whose v2 payloads put long content before the tokens', async (t) => {
		const root = await mkdtemp(path.join(tmpdir(), 'ccusage-fixture-test-'));
		t.after(async () => rm(root, { recursive: true, force: true }));
		const opencodeDir = path.join(root, 'opencode');

		const result = spawnSync(
			process.execPath,
			[
				generator,
				'--output-dir',
				path.join(root, 'claude'),
				'--size-mib',
				'1',
				'--opencode-output-dir',
				opencodeDir,
				'--opencode-size-mib',
				'8',
			],
			{ encoding: 'utf8' },
		);
		assert.equal(result.status, 0, result.stderr);

		const dbPath = path.join(opencodeDir, 'opencode.db');
		const db = new DatabaseSync(dbPath, { readOnly: true });
		t.after(() => db.close());
		assert.equal(db.prepare('PRAGMA journal_mode').get()?.journal_mode, 'wal');
		assert.ok(
			db
				.prepare("SELECT 1 FROM sqlite_master WHERE name = 'session_message_time_created_idx'")
				.get(),
		);
		const rows = db
			.prepare('SELECT type, time_created, data FROM session_message ORDER BY rowid')
			.all();
		const assistants = rows.filter((row) => row.type === 'assistant');
		assert.ok(rows.some((row) => row.type === 'user'));
		assert.ok(assistants.length >= 200, `Assistant rows: ${assistants.length}`);

		let largest = 0;
		for (const row of assistants) {
			assert.equal(typeof row.data, 'string');
			const data = String(row.data);
			largest = Math.max(largest, data.length);
			const payload: unknown = JSON.parse(data);
			assert.ok(isRow(payload) && isRow(payload.time));
			assert.equal(payload.time.created, row.time_created);
			const keys = Object.keys(payload);
			assert.ok(keys.indexOf('content') < keys.indexOf('tokens'), keys.join(','));
			assert.ok(data.indexOf('"tokens"') > data.length / 2, 'tokens sit after the content');
		}
		assert.ok(largest > 32_000 && largest <= 49_152, `Largest payload: ${largest}`);
		const { size } = await stat(dbPath);
		assert.ok(size > 7 * 1024 * 1024 && size < 10 * 1024 * 1024, `Database size: ${size}`);
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
