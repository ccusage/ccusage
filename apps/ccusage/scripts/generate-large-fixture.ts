#!/usr/bin/env nix
/*
#! nix shell --inputs-from ../../.. nixpkgs#bun -c bun
*/
import { dirname, join, resolve } from 'node:path';

const MIB = 1024 * 1024;
const CHUNK_LINE_COUNT = 128;
const REAL_WORLD_PROFILE_FILES = 3142;
const REAL_WORLD_PROFILE_TOTAL_MIB = 1238.9718046188354;
const REAL_WORLD_QUANTILES = [
	{ percentile: 0, size: 236 },
	{ percentile: 0.5, size: 105_267 },
	{ percentile: 0.75, size: 233_572 },
	{ percentile: 0.9, size: 653_972 },
	{ percentile: 0.95, size: 1_504_757 },
	{ percentile: 0.99, size: 5_383_751 },
	{ percentile: 1, size: 87_033_471 },
] as const;

type FixtureResult = {
	fileCount: number;
	lineCount: number;
	totalBytes: number;
};

type Options = {
	outputDir: string;
	codexOutputDir?: string;
	sizeMib: number;
	codexSizeMib: number;
};

function parsePositiveInteger(value: string, flagName: string): number {
	const parsed = Number(value);
	if (!Number.isSafeInteger(parsed) || parsed < 1) {
		throw new Error(`${flagName} must be a positive integer`);
	}
	return parsed;
}

function parseArguments(args: readonly string[]): Options {
	const values = new Map<string, string>();
	for (let index = 0; index < args.length; index += 1) {
		const flag = args[index];
		if (!flag?.startsWith('--')) {
			throw new Error(`Unexpected argument: ${flag ?? ''}`);
		}
		const value = args[index + 1];
		if (!value || value.startsWith('--')) {
			throw new Error(`${flag} requires a value`);
		}
		if (!['--output-dir', '--codex-output-dir', '--size-mib', '--codex-size-mib'].includes(flag)) {
			throw new Error(`Unknown option: ${flag}`);
		}
		values.set(flag, value);
		index += 1;
	}

	const outputDir = values.get('--output-dir');
	if (!outputDir) {
		throw new Error('--output-dir is required');
	}

	return {
		outputDir,
		codexOutputDir: values.get('--codex-output-dir'),
		sizeMib: parsePositiveInteger(values.get('--size-mib') ?? '1024', '--size-mib'),
		codexSizeMib: parsePositiveInteger(
			values.get('--codex-size-mib') ?? '1024',
			'--codex-size-mib',
		),
	};
}

function formatBytes(bytes: number): string {
	return `${(bytes / MIB).toFixed(2)} MiB`;
}

function interpolateFileSize(percentile: number): number {
	for (let index = 1; index < REAL_WORLD_QUANTILES.length; index += 1) {
		const previous = REAL_WORLD_QUANTILES[index - 1];
		const current = REAL_WORLD_QUANTILES[index];
		if (!previous || !current || percentile > current.percentile) {
			continue;
		}
		const ratio = (percentile - previous.percentile) / (current.percentile - previous.percentile);
		return previous.size + (current.size - previous.size) * ratio;
	}
	return REAL_WORLD_QUANTILES.at(-1)?.size ?? 0;
}

function createFileSizeTargets(targetBytes: number): number[] {
	const targetFileCount = Math.max(
		1,
		Math.round((targetBytes / MIB / REAL_WORLD_PROFILE_TOTAL_MIB) * REAL_WORLD_PROFILE_FILES),
	);
	const rawSizes = Array.from({ length: targetFileCount }, (_, index) =>
		interpolateFileSize((index + 0.5) / targetFileCount),
	);
	const rawTotal = rawSizes.reduce((total, size) => total + size, 0);
	return rawSizes.map((size) => Math.max(256, Math.round(size * (targetBytes / rawTotal))));
}

function shuffledIndex(index: number, length: number): number {
	return (index * (length - 1) + 17) % length;
}

function contentLength(index: number): number {
	if (index % 997 === 0) {
		return 48 * 1024 + (index % (32 * 1024));
	}
	if (index % 37 === 0) {
		return 8 * 1024 + (index % (8 * 1024));
	}
	return 1800 + (((index * 1_103_515_245 + 12_345) % 4_294_967_296) % 2400);
}

function paddedNumber(value: number, width: number): string {
	return String(value).padStart(width, '0');
}

function createClaudeUsageLine(index: number, fileIndex: number, sessionId: string): string {
	const padding = 'x'.repeat(contentLength(index));
	const payload = {
		timestamp: `2026-01-${paddedNumber((index % 28) + 1, 2)}T${paddedNumber(index % 24, 2)}:${paddedNumber(Math.floor(index / 24) % 60, 2)}:00.000Z`,
		cwd: `/tmp/ccusage-large-fixture/project-${paddedNumber(fileIndex % 128, 3)}`,
		sessionId,
		version: '1.0.0',
		message: {
			id: `msg_${index.toString(36).padStart(10, '0')}`,
			model: index % 5 === 0 ? 'claude-opus-4-20250514' : 'claude-sonnet-4-20250514',
			content: [{ type: 'text', text: padding }],
			usage: {
				input_tokens: 100 + (index % 1000),
				output_tokens: 20 + (index % 200),
				cache_creation_input_tokens: index % 300,
				cache_read_input_tokens: index % 5000,
				...(index % 7 === 0 ? { speed: 'fast' } : {}),
			},
		},
		requestId: `req_${index.toString(36).padStart(10, '0')}`,
	};
	return `${JSON.stringify(payload)}\n`;
}

function createCodexUsageLine(index: number, fileIndex: number): string {
	const inputTokens = 200 + (index % 2000);
	const outputTokens = 40 + (index % 600);
	const reasoningOutputTokens = index % 300;
	const totalTokens = inputTokens + outputTokens + reasoningOutputTokens;
	const usage = {
		input_tokens: inputTokens,
		cached_input_tokens: index % 1200,
		output_tokens: outputTokens,
		reasoning_output_tokens: reasoningOutputTokens,
		total_tokens: totalTokens,
	};
	return `${JSON.stringify({
		timestamp: `2026-01-${paddedNumber((index % 28) + 1, 2)}T${paddedNumber(index % 24, 2)}:${paddedNumber(Math.floor(index / 24) % 60, 2)}:00.000Z`,
		type: 'event_msg',
		payload: {
			type: 'token_count',
			info: {
				model: index % 5 === 0 ? 'gpt-5.3-codex' : 'gpt-5.2-codex',
				last_token_usage: usage,
				total_token_usage: usage,
			},
			content: 'x'.repeat(contentLength(index + fileIndex)),
		},
	})}\n`;
}

function assertSafeDeletionTarget(directory: string, flagName: string): string {
	const resolved = resolve(directory);
	if (resolved === resolve('/') || resolved === resolve('.') || resolved.length < 5) {
		throw new Error(`Refusing to delete unsafe ${flagName} path: ${resolved}`);
	}
	return resolved;
}

async function generateFixture(
	outputDir: string,
	sizeMib: number,
	format: 'claude' | 'codex',
): Promise<FixtureResult> {
	const directory = assertSafeDeletionTarget(
		outputDir,
		format === 'claude' ? '--output-dir' : '--codex-output-dir',
	);
	const fileSizeTargets = createFileSizeTargets(sizeMib * MIB);
	await Bun.$`rm -rf ${directory}`;

	let totalBytes = 0;
	let lineIndex = 0;
	for (let fileIndex = 0; fileIndex < fileSizeTargets.length; fileIndex += 1) {
		const targetSize = fileSizeTargets[shuffledIndex(fileIndex, fileSizeTargets.length)];
		if (targetSize === undefined) {
			throw new Error(`Missing file size target for index ${fileIndex}`);
		}
		const projectName = `project-${paddedNumber(fileIndex % 128, 3)}`;
		const sessionId = `session-${paddedNumber(fileIndex, 6)}`;
		const filePath =
			format === 'claude'
				? join(directory, 'projects', projectName, `${sessionId}.jsonl`)
				: join(directory, 'sessions', projectName, `${sessionId}.jsonl`);
		await Bun.$`mkdir -p ${dirname(filePath)}`;

		const writer = Bun.file(filePath).writer();
		let fileBytes = 0;
		while (fileBytes < targetSize) {
			let chunk = '';
			for (
				let chunkIndex = 0;
				chunkIndex < CHUNK_LINE_COUNT && fileBytes + Buffer.byteLength(chunk) < targetSize;
				chunkIndex += 1
			) {
				chunk +=
					format === 'claude'
						? createClaudeUsageLine(lineIndex, fileIndex, sessionId)
						: createCodexUsageLine(lineIndex, fileIndex);
				lineIndex += 1;
			}
			const chunkBytes = Buffer.byteLength(chunk);
			await writer.write(chunk);
			fileBytes += chunkBytes;
			totalBytes += chunkBytes;
		}
		await writer.end();
	}

	return { fileCount: fileSizeTargets.length, lineCount: lineIndex, totalBytes };
}

function printResult(name: string, outputDir: string, result: FixtureResult): void {
	console.log(`Generated ${name} fixture ${outputDir}`);
	console.log(`Files: ${result.fileCount}`);
	console.log(`Rows: ${result.lineCount}`);
	console.log(`Size: ${formatBytes(result.totalBytes)}`);
}

async function main(): Promise<void> {
	const options = parseArguments(process.argv.slice(2));
	const outputDir = resolve(options.outputDir);
	const claudeResult = await generateFixture(outputDir, options.sizeMib, 'claude');
	printResult('Claude', outputDir, claudeResult);

	if (options.codexOutputDir) {
		const codexOutputDir = resolve(options.codexOutputDir);
		const codexResult = await generateFixture(codexOutputDir, options.codexSizeMib, 'codex');
		printResult('Codex', codexOutputDir, codexResult);
	}
}

await main();
