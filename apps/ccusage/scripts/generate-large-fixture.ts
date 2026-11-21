#!/usr/bin/env nix
/*
#! nix shell --inputs-from ../../.. nixpkgs#nodejs -c node
*/
import { mkdir, readdir, stat, utimes, writeFile } from 'node:fs/promises';
import { dirname, join, resolve } from 'node:path';
import { DatabaseSync } from 'node:sqlite';
import { zstdCompressSync } from 'node:zlib';

const MIB = 1024 * 1024;
const DAY_MS = 24 * 60 * 60 * 1000;
const FIXTURE_START_MS = Date.UTC(2026, 5, 1);
const FIXTURE_DAYS = 120;
const CLAUDE_PROJECT_COUNT = 48;
const CLAUDE_MANY_SMALL_FILES_PROJECT_COUNT = 1600;
const SUBAGENT_BURST_EVERY = 16;
const SUBAGENT_BURST_FILES = 4;
const FILE_HISTORY_TRACKED_FILES = 160;
const LEGACY_PROGRESS_FILE_EVERY = 6;
const LEGACY_PROGRESS_USAGE_CHANCE = 0.08;
const METADATA_FIRST_ASSISTANT_CHANCE = 0.01;
const CODEX_PLAIN_RECENT_DAYS = 14;
const CODEX_ARCHIVED_FILE_EVERY = 4;
const CODEX_COMPACTION_CHANCE = 1 / 30;
const CODEX_FORK_FILE_EVERY = 8;
const CODEX_FORK_HISTORY_LINES = 120;
const OPENCODE_DEFAULT_SIZE_MIB = 1024;
const OPENCODE_SESSION_ROWS = 400;
const OPENCODE_MAX_PAYLOAD_BYTES = 48 * 1024;
type Format = 'claude' | 'codex' | 'grok';
type ClaudeShape = 'default' | 'many-small-files' | 'requestless-proxy';
const CLAUDE_SHAPES = [
	'default',
	'many-small-files',
	'requestless-proxy',
] as const satisfies readonly ClaudeShape[];

type SizeProfile = {
	files: number;
	totalBytes: number;
	quantiles: readonly { percentile: number; size: number }[];
};
const SIZE_PROFILES = {
	claude: {
		files: 10_723,
		totalBytes: 11_084_634_346,
		quantiles: [
			{ percentile: 0, size: 20 },
			{ percentile: 0.1, size: 12_650 },
			{ percentile: 0.25, size: 165_359 },
			{ percentile: 0.5, size: 456_592 },
			{ percentile: 0.75, size: 982_483 },
			{ percentile: 0.9, size: 1_986_849 },
			{ percentile: 0.95, size: 3_239_049 },
			{ percentile: 0.98, size: 6_636_290 },
			{ percentile: 0.99, size: 10_975_928 },
			{ percentile: 0.995, size: 16_288_675 },
			{ percentile: 0.999, size: 38_777_640 },
			{ percentile: 1, size: 129_659_313 },
		],
	},
	codex: {
		files: 7312,
		totalBytes: 15_293_400_760,
		quantiles: [
			{ percentile: 0, size: 20_070 },
			{ percentile: 0.1, size: 163_312 },
			{ percentile: 0.25, size: 356_202 },
			{ percentile: 0.5, size: 797_068 },
			{ percentile: 0.75, size: 1_610_200 },
			{ percentile: 0.9, size: 3_762_690 },
			{ percentile: 0.95, size: 6_397_926 },
			{ percentile: 0.98, size: 12_601_686 },
			{ percentile: 0.99, size: 23_663_548 },
			{ percentile: 0.995, size: 39_980_016 },
			{ percentile: 0.999, size: 99_553_853 },
			{ percentile: 1, size: 221_490_002 },
		],
	},
	grok: {
		files: 354,
		totalBytes: 98_732_436,
		quantiles: [
			{ percentile: 0, size: 601 },
			{ percentile: 0.1, size: 4671 },
			{ percentile: 0.25, size: 6147 },
			{ percentile: 0.5, size: 7937 },
			{ percentile: 0.75, size: 153_184 },
			{ percentile: 0.9, size: 1_012_536 },
			{ percentile: 0.95, size: 1_301_281 },
			{ percentile: 0.98, size: 2_281_428 },
			{ percentile: 0.99, size: 2_906_443 },
			{ percentile: 0.995, size: 4_749_250 },
			{ percentile: 0.999, size: 5_332_993 },
			{ percentile: 1, size: 5_332_993 },
		],
	},
} as const satisfies Record<Format, SizeProfile>;
const CLAUDE_MANY_SMALL_FILES_PROFILE = {
	files: 40_000,
	totalBytes: 512 * MIB,
	quantiles: [
		{ percentile: 0, size: 400 },
		{ percentile: 0.25, size: 3000 },
		{ percentile: 0.5, size: 8000 },
		{ percentile: 0.75, size: 16_000 },
		{ percentile: 0.9, size: 32_000 },
		{ percentile: 0.99, size: 120_000 },
		{ percentile: 1, size: 400_000 },
	],
} as const satisfies SizeProfile;

function sizeProfile(format: Format, claudeShape: ClaudeShape): SizeProfile {
	return format === 'claude' && claudeShape === 'many-small-files'
		? CLAUDE_MANY_SMALL_FILES_PROFILE
		: SIZE_PROFILES[format];
}

const CLAUDE_MODELS = [
	'claude-opus-4-5-20251101',
	'claude-opus-4-5-20251101',
	'claude-sonnet-4-5-20250929',
	'claude-haiku-4-5-20251001',
] as const;
const GROK_MODELS = ['grok-4.5-build', 'grok-4.5-build', 'grok-4.3-build'] as const;
const CODEX_MODELS = [
	'gpt-5.2-codex',
	'gpt-5.2-codex',
	'gpt-5.3-codex',
	'gpt-5.1-codex-mini',
] as const;
const BMP_WORDS = [
	'the',
	'parser',
	'returns',
	'a',
	'value',
	'when',
	'usage',
	'record',
	'is',
	'missing',
	'from',
	'line',
	'fn',
	'let',
	'match',
	'Some(x)',
	'=>',
	'{',
	'}',
	'"quoted"',
	'path/to/file.rs',
	'C:\\dir\\file',
	'\t',
	'café',
	'日本語',
	'—',
	'<tag attr="1">',
	'0x1f',
	'TODO:',
	'error[E0308]:',
] as const;
const CLAUDE_PLAIN_WORDS = BMP_WORDS.filter((word) => !/["\\]|[^\p{ASCII}]|\t/u.test(word));

type FixtureResult = {
	fileCount: number;
	lineCount: number;
	totalBytes: number;
	diskBytes: number;
};

type CodexLayout = 'current' | 'legacy';

type Options = {
	outputDir: string;
	codexOutputDir?: string;
	grokOutputDir?: string;
	opencodeOutputDir?: string;
	sizeMib: number;
	codexSizeMib: number;
	grokSizeMib: number;
	opencodeSizeMib: number;
	codexLayout: CodexLayout;
	claudeShape: ClaudeShape;
};

type Line = Record<string, unknown>;

function parsePositiveInteger(value: string, flagName: string): number {
	const parsed = Number(value);
	if (!Number.isSafeInteger(parsed) || parsed < 1) {
		throw new Error(`${flagName} must be a positive integer`);
	}
	return parsed;
}

function parseSizeMib(value: string | undefined, flagName: string, profile: SizeProfile): number {
	return value === undefined
		? Math.round(profile.totalBytes / MIB)
		: parsePositiveInteger(value, flagName);
}

function isClaudeShape(value: string): value is ClaudeShape {
	return CLAUDE_SHAPES.some((shape) => shape === value);
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
		if (
			![
				'--output-dir',
				'--codex-output-dir',
				'--grok-output-dir',
				'--size-mib',
				'--codex-size-mib',
				'--grok-size-mib',
				'--opencode-output-dir',
				'--opencode-size-mib',
				'--codex-layout',
				'--claude-shape',
			].includes(flag)
		) {
			throw new Error(`Unknown option: ${flag}`);
		}
		values.set(flag, value);
		index += 1;
	}

	const outputDir = values.get('--output-dir');
	if (!outputDir) {
		throw new Error('--output-dir is required');
	}
	const codexLayout = values.get('--codex-layout') ?? 'current';
	if (codexLayout !== 'current' && codexLayout !== 'legacy') {
		throw new Error('--codex-layout must be current or legacy');
	}
	const claudeShape = values.get('--claude-shape') ?? 'default';
	if (!isClaudeShape(claudeShape)) {
		throw new Error(`--claude-shape must be one of ${CLAUDE_SHAPES.join(', ')}`);
	}

	return {
		outputDir,
		codexOutputDir: values.get('--codex-output-dir'),
		grokOutputDir: values.get('--grok-output-dir'),
		sizeMib: parseSizeMib(
			values.get('--size-mib'),
			'--size-mib',
			sizeProfile('claude', claudeShape),
		),
		codexSizeMib: parseSizeMib(
			values.get('--codex-size-mib'),
			'--codex-size-mib',
			SIZE_PROFILES.codex,
		),
		grokSizeMib: parseSizeMib(values.get('--grok-size-mib'), '--grok-size-mib', SIZE_PROFILES.grok),
		opencodeOutputDir: values.get('--opencode-output-dir'),
		opencodeSizeMib: parsePositiveInteger(
			values.get('--opencode-size-mib') ?? String(OPENCODE_DEFAULT_SIZE_MIB),
			'--opencode-size-mib',
		),
		codexLayout,
		claudeShape,
	};
}

function formatBytes(bytes: number): string {
	return `${(bytes / MIB).toFixed(2)} MiB`;
}

function interpolateFileSize(profile: SizeProfile, percentile: number): number {
	const { quantiles } = profile;
	for (let index = 1; index < quantiles.length; index += 1) {
		const previous = quantiles[index - 1];
		const current = quantiles[index];
		if (!previous || !current || percentile > current.percentile) {
			continue;
		}
		const ratio = (percentile - previous.percentile) / (current.percentile - previous.percentile);
		return Math.expm1(
			Math.log1p(previous.size) + (Math.log1p(current.size) - Math.log1p(previous.size)) * ratio,
		);
	}
	return quantiles.at(-1)?.size ?? 0;
}

function createFileSizeTargets(profile: SizeProfile, targetBytes: number): number[] {
	const targetFileCount = Math.max(
		1,
		Math.round((targetBytes / profile.totalBytes) * profile.files),
	);
	const rawSizes = Array.from({ length: targetFileCount }, (_, index) =>
		interpolateFileSize(profile, (index + 0.5) / targetFileCount),
	);
	const rawTotal = rawSizes.reduce((total, size) => total + size, 0);
	return rawSizes.map((size) => Math.max(256, Math.round(size * (targetBytes / rawTotal))));
}

function shuffledIndex(index: number, length: number): number {
	return (index * (length - 1) + 17) % length;
}

class Random {
	#state: number;

	constructor(seed: number) {
		this.#state = seed >>> 0;
	}

	next(): number {
		this.#state = (this.#state + 0x6d2b79f5) >>> 0;
		let value = this.#state;
		value = Math.imul(value ^ (value >>> 15), value | 1);
		value ^= value + Math.imul(value ^ (value >>> 7), value | 61);
		return ((value ^ (value >>> 14)) >>> 0) / 4_294_967_296;
	}

	int(maxExclusive: number): number {
		return Math.floor(this.next() * maxExclusive);
	}

	between(min: number, max: number): number {
		return min + this.int(max - min + 1);
	}

	chance(probability: number): boolean {
		return this.next() < probability;
	}

	pick<T>(values: readonly T[]): T {
		const value = values[this.int(values.length)];
		if (value === undefined) {
			throw new Error('Cannot pick from an empty list');
		}
		return value;
	}

	hex(length: number): string {
		let output = '';
		while (output.length < length) {
			output += this.int(4_294_967_296).toString(16).padStart(8, '0');
		}
		return output.slice(0, length);
	}

	uuid(): string {
		const hex = this.hex(32);
		return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-4${hex.slice(13, 16)}-a${hex.slice(17, 20)}-${hex.slice(20, 32)}`;
	}
}

function claudeAssistantWord(random: Random): string {
	// NOTE(2026-11-19, Claude Code 2.1, #2221): Usage bodies average about eight JSON escapes per KiB and 3% non-ASCII bytes. The shared transcript pool would overstate the cost of skipping them.
	const sample = random.int(1000);
	if (sample < 11) {
		return '"quoted"';
	}
	if (sample < 35) {
		return '日本語';
	}
	if (sample < 62) {
		return '\n';
	}
	if (sample < 71) {
		return '\t';
	}
	return random.pick(CLAUDE_PLAIN_WORDS);
}

function buildTextPool(seed: number, pickWord: (random: Random) => string): string {
	const random = new Random(seed);
	const parts: string[] = [];
	let length = 0;
	while (length < 2 * MIB) {
		const word = pickWord(random);
		parts.push(word, ' ');
		length += word.length + 1;
	}
	return parts.join('');
}

function buildOpaquePool(): string {
	const alphabet = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
	const random = new Random(11);
	return Array.from({ length: MIB }, () => alphabet[random.int(alphabet.length)]).join('');
}

const BMP_TEXT_POOL = buildTextPool(7, (random) =>
	random.chance(0.08) ? '\n' : random.pick(BMP_WORDS),
);
const CLAUDE_ASSISTANT_TEXT_POOL = buildTextPool(13, claudeAssistantWord);
const OPAQUE_POOL = buildOpaquePool();

function text(random: Random, length: number, pool = BMP_TEXT_POOL): string {
	const start = random.int(pool.length - length - 1);
	return pool.slice(start, start + length);
}

function opaque(random: Random, length: number): string {
	const start = random.int(OPAQUE_POOL.length - length - 1);
	return OPAQUE_POOL.slice(start, start + length);
}

function around(random: Random, average: number): number {
	return Math.max(16, Math.round(average * (0.3 + random.next() * 1.4)));
}

class Clock {
	#ms: number;

	constructor(startMs: number) {
		this.#ms = startMs;
	}

	tick(random: Random, maxMs = 20_000): string {
		this.#ms += 50 + random.int(maxMs);
		return new Date(this.#ms).toISOString();
	}
}

type ClaudeSession = {
	clock: Clock;
	cwd: string;
	projectDir: string;
	sessionId: string;
	version: string;
	agentId?: string;
	legacyProgress: boolean;
	parentMessageIds: string[];
	random: Random;
	lastUuid?: string;
	proxy?: { constantMessageId?: string; messageIdPrefix: string };
};

function claudeEnvelope(
	session: ClaudeSession,
	type: string,
	timestamp: string,
	assistant?: { message: Line; requestId?: string },
): Line {
	const ancestry = {
		parentUuid: session.lastUuid ?? null,
		isSidechain: session.agentId !== undefined,
	};
	const agent = session.agentId === undefined ? {} : { agentId: session.agentId };
	const context = {
		userType: 'external',
		entrypoint: 'cli',
		cwd: session.cwd,
		sessionId: session.sessionId,
		version: session.version,
		gitBranch: 'main',
	};
	const record = { type, uuid: session.random.uuid(), timestamp };
	session.lastUuid = record.uuid;
	if (assistant && !session.random.chance(METADATA_FIRST_ASSISTANT_CHANCE)) {
		// NOTE(2026-11-19, Claude Code 2.1, #2221): The message precedes report metadata in most usage rows. Preserve insertion order to exercise this parser path.
		return { ...ancestry, ...assistant, ...record, ...context, ...agent };
	}
	return { ...ancestry, ...agent, ...context, ...record, ...assistant };
}

function claudeUsage(random: Random, model: string): Line {
	const input = random.between(1, 40);
	const cacheRead = random.between(0, 180_000);
	const fiveMinute = random.chance(0.3) ? random.between(0, 4000) : 0;
	const oneHour = random.chance(0.6) ? random.between(0, 30_000) : 0;
	const output = random.between(1, 4000);
	const cacheCreation = {
		ephemeral_5m_input_tokens: fiveMinute,
		ephemeral_1h_input_tokens: oneHour,
	};
	const iterations: Line[] = [
		{
			input_tokens: input,
			output_tokens: output,
			cache_read_input_tokens: cacheRead,
			cache_creation_input_tokens: fiveMinute + oneHour,
			cache_creation: cacheCreation,
			type: 'message',
		},
	];
	if (random.chance(0.03)) {
		iterations.push({
			input_tokens: random.between(100, 4000),
			output_tokens: random.between(10, 800),
			cache_read_input_tokens: 0,
			cache_creation_input_tokens: 0,
			cache_creation: { ephemeral_5m_input_tokens: 0, ephemeral_1h_input_tokens: 0 },
			type: 'advisor_message',
			model: model === CLAUDE_MODELS[0] ? CLAUDE_MODELS[2] : CLAUDE_MODELS[0],
		});
	}
	return {
		input_tokens: input,
		cache_creation_input_tokens: fiveMinute + oneHour,
		cache_read_input_tokens: cacheRead,
		output_tokens: output,
		output_tokens_details: { thinking_tokens: random.between(0, output) },
		server_tool_use: { web_search_requests: 0, web_fetch_requests: 0 },
		service_tier: 'standard',
		cache_creation: cacheCreation,
		inference_geo: 'global',
		iterations,
		speed: random.chance(0.1) ? 'fast' : 'standard',
		fallback_credit: null,
	};
}

function claudeContentBlock(random: Random, kind: string): Line {
	switch (kind) {
		case 'thinking':
			return {
				type: 'thinking',
				thinking: text(random, around(random, 1600), CLAUDE_ASSISTANT_TEXT_POOL),
				signature: opaque(random, around(random, 600)),
			};
		case 'tool_use':
			return {
				type: 'tool_use',
				id: `toolu_${random.hex(24)}`,
				name: random.pick(['Bash', 'Read', 'Edit', 'Grep', 'Task']),
				input: {
					command: text(random, around(random, 900), CLAUDE_ASSISTANT_TEXT_POOL),
					description: text(random, 60, CLAUDE_ASSISTANT_TEXT_POOL),
				},
				caller: { type: 'direct' },
			};
		default:
			return {
				type: 'text',
				text: text(random, around(random, 1800), CLAUDE_ASSISTANT_TEXT_POOL),
			};
	}
}

function claudeAssistantResponse(
	session: ClaudeSession,
	messageId?: string,
): { lines: Line[]; replayableId?: string } {
	const { random } = session;
	const model = random.chance(0.01) ? '<synthetic>' : random.pick(CLAUDE_MODELS);
	const { proxy } = session;
	const id =
		messageId ?? proxy?.constantMessageId ?? `${proxy?.messageIdPrefix ?? 'msg_'}${random.hex(24)}`;
	const requestId = proxy === undefined ? { requestId: `req_${random.hex(24)}` } : {};
	const usage = claudeUsage(random, model);
	const blocks = random.chance(0.6) ? ['thinking'] : [];
	blocks.push(random.chance(0.55) ? 'tool_use' : 'text');
	if (random.chance(0.3)) {
		blocks.push('tool_use');
	}
	const timestamp = session.clock.tick(random);
	const lines: Line[] = blocks.map((kind) => {
		const message = {
			model,
			id,
			type: 'message',
			role: 'assistant',
			content: [claudeContentBlock(random, kind)],
			container: null,
			stop_reason: kind === 'tool_use' ? 'tool_use' : 'end_turn',
			stop_sequence: null,
			stop_details: null,
			usage,
			input_transformations: [],
			diagnostics: null,
			context_management: null,
		};
		if (
			session.legacyProgress &&
			session.agentId === undefined &&
			random.chance(LEGACY_PROGRESS_USAGE_CHANCE)
		) {
			return {
				...claudeEnvelope(session, 'progress', timestamp),
				data: {
					message: { type: 'assistant', message, ...requestId, uuid: random.uuid(), timestamp },
					normalizedMessages: [],
					type: 'agent_progress',
					agentId: random.hex(17),
				},
				toolUseID: `toolu_${random.hex(24)}`,
				parentToolUseID: `toolu_${random.hex(24)}`,
			};
		}
		return {
			...claudeEnvelope(session, 'assistant', timestamp, { message, ...requestId }),
			requestedModel: model,
			effort: 'high',
			apiBlockIndex: 0,
			promptCacheRequested: true,
			advisorModel: null,
		};
	});
	return { lines, replayableId: lines[0]?.type === 'assistant' ? id : undefined };
}

function claudeUser(session: ClaudeSession, withToolResult: boolean): Line {
	const { random } = session;
	const timestamp = session.clock.tick(random);
	if (!withToolResult) {
		return {
			...claudeEnvelope(session, 'user', timestamp),
			message: { role: 'user', content: text(random, around(random, 1200)) },
			promptId: random.uuid(),
			permissionMode: 'default',
		};
	}
	const output = text(random, around(random, 2600));
	return {
		...claudeEnvelope(session, 'user', timestamp),
		message: {
			role: 'user',
			content: [
				{
					tool_use_id: `toolu_${random.hex(24)}`,
					type: 'tool_result',
					content: output,
					is_error: false,
				},
			],
		},
		toolUseResult: {
			stdout: output,
			stderr: '',
			interrupted: false,
			isImage: false,
			noOutputExpected: null,
		},
		sourceToolAssistantUUID: random.uuid(),
		sandboxed: false,
		permissionDecision: null,
	};
}

function claudeSideLines(session: ClaudeSession): Line[] {
	const { random } = session;
	const timestamp = session.clock.tick(random, 200);
	const lines: Line[] = [];
	if (random.chance(0.9)) {
		lines.push({
			...claudeEnvelope(session, 'attachment', timestamp),
			attachment: {
				type: 'file',
				filename: text(random, 40),
				content: text(random, around(random, 3300)),
			},
			rendered: random.chance(0.5) ? null : 'inline',
		});
	}
	if (random.chance(0.45)) {
		lines.push({
			type: 'api-request',
			id: random.uuid(),
			sessionId: session.sessionId,
			version: session.version,
			timestamp,
			querySource: 'repl_main_thread',
			requestDigest: random.hex(64),
			shapeHash: random.hex(16),
			base: { system: text(random, around(random, 2600)) },
			tail: [{ role: 'user', content: text(random, around(random, 1200)) }],
			params: { max_tokens: 32_000, thinking: { type: 'enabled' } },
			cacheControl: { ttl: '1h' },
			keep: random.int(40),
		});
	}
	if (random.chance(0.3)) {
		lines.push({
			type: 'api-request-blob',
			hash: random.hex(64),
			sessionId: session.sessionId,
			message: { role: 'user', content: text(random, around(random, 4800)) },
		});
	}
	if (random.chance(0.15)) {
		lines.push({
			type: 'last-prompt',
			lastPrompt: text(random, 120),
			leafUuid: random.uuid(),
			sessionId: session.sessionId,
		});
	}
	if (random.chance(0.1)) {
		lines.push({
			type: 'queue-operation',
			operation: 'enqueue',
			timestamp,
			sessionId: session.sessionId,
			content: text(random, 600),
		});
	}
	if (random.chance(0.06)) {
		lines.push({ type: 'mode', mode: 'normal', sessionId: session.sessionId });
	}
	if (session.legacyProgress && random.chance(0.5)) {
		lines.push({
			...claudeEnvelope(session, 'progress', timestamp),
			data: {
				type: 'hook_progress',
				hookEvent: 'PostToolUse',
				hookName: 'PostToolUse:Bash',
				command: text(random, 80),
			},
			parentToolUseID: `toolu_${random.hex(24)}`,
			toolUseID: `toolu_${random.hex(24)}`,
		});
	}
	return lines;
}

function* claudeSessionLines(session: ClaudeSession): Generator<Line> {
	const { random } = session;
	yield {
		type: 'session-owner',
		accountUuid: random.uuid(),
		organizationUuid: random.uuid(),
		sessionId: session.sessionId,
	};
	yield { type: 'custom-title', customTitle: text(random, 40), sessionId: session.sessionId };
	for (const messageId of session.parentMessageIds.slice(0, 2)) {
		yield* claudeAssistantResponse(session, messageId).lines;
	}
	while (true) {
		yield claudeUser(session, false);
		const steps = random.between(1, 6);
		for (let step = 0; step < steps; step += 1) {
			yield* claudeSideLines(session);
			const response = claudeAssistantResponse(session);
			if (
				session.agentId === undefined &&
				response.replayableId !== undefined &&
				session.parentMessageIds.length < 4
			) {
				session.parentMessageIds.push(response.replayableId);
			}
			yield* response.lines;
			yield claudeUser(session, true);
		}
	}
}

type CodexSession = {
	clock: Clock;
	cwd: string;
	sessionId: string;
	random: Random;
	ordinal: number;
	total: Line;
	fork?: CodexFork;
};

type CodexParent = { sessionId: string; history: Line[] };
type CodexFork = { parent: CodexParent; spawned: boolean };

function codexUsage(random: Random): Line {
	const input = random.between(2000, 180_000);
	const output = random.between(20, 6000);
	const reasoning = random.between(0, output);
	return {
		input_tokens: input,
		cached_input_tokens: Math.floor(input * random.next()),
		cache_write_input_tokens: 0,
		output_tokens: output,
		reasoning_output_tokens: reasoning,
		total_tokens: input + output,
	};
}

function addUsage(total: Line, usage: Line): Line {
	return Object.fromEntries(
		Object.entries(usage).map(([key, value]) => [key, Number(total[key] ?? 0) + Number(value)]),
	);
}

function codexLine(session: CodexSession, type: string, payload: Line, maxMs?: number): Line {
	session.ordinal += 1;
	return {
		timestamp: session.clock.tick(session.random, maxMs),
		ordinal: session.ordinal,
		type,
		payload,
	};
}

function codexItemCompleted(session: CodexSession, turnId: string): Line {
	const { random } = session;
	return codexLine(session, 'event_msg', {
		type: 'item_completed',
		thread_id: session.sessionId,
		turn_id: turnId,
		item: {
			type: 'command_execution',
			id: random.uuid(),
			process_id: String(random.int(90_000)),
			command: ['bash', '-lc', text(random, 120)],
			cwd: session.cwd,
			parsed_cmd: [{ type: 'unknown', cmd: text(random, 80) }],
			source: 'agent',
			status: 'completed',
			aggregated_output: text(random, around(random, 2600)),
			exit_code: 0,
			duration: { secs: random.int(20), nanos: random.int(1_000_000_000) },
		},
		started_at_ms: FIXTURE_START_MS + session.ordinal,
		completed_at_ms: FIXTURE_START_MS + session.ordinal + 1,
	});
}

function codexUsageRecordPayload(session: CodexSession, turnId: string, usage: Line) {
	return {
		thread_id: session.sessionId,
		turn_id: turnId,
		session_id: session.sessionId,
		root_turn_id: turnId,
		response_id: `resp_${session.random.hex(32)}`,
		usage,
		turn_token_usage: usage,
		thread_token_usage: session.total,
	};
}

function codexForkSource(fork: CodexFork): Line | string {
	return fork.spawned
		? { subagent: { thread_spawn: { parent_thread_id: fork.parent.sessionId, depth: 1 } } }
		: 'cli';
}

function* codexCopiedHistory(session: CodexSession, fork: CodexFork): Generator<Line> {
	const { history } = fork.parent;
	const copied = history.slice(1, 1 + session.random.between(1, Math.max(1, history.length - 1)));
	const forkedAt = session.clock.tick(session.random, 50);
	for (const line of copied) {
		if (fork.spawned && line.type === 'token_usage_record') {
			continue;
		}
		session.ordinal += 1;
		yield { ...line, timestamp: forkedAt, ordinal: session.ordinal };
	}
}

function* codexSessionLines(session: CodexSession): Generator<Line> {
	const { random, fork } = session;
	yield codexLine(session, 'session_meta', {
		session_id: session.sessionId,
		id: session.sessionId,
		...(fork === undefined ? {} : { forked_from_id: fork.parent.sessionId }),
		timestamp: new Date(FIXTURE_START_MS).toISOString(),
		cwd: session.cwd,
		originator: 'codex_cli_rs',
		cli_version: '0.200.0',
		source: fork === undefined ? 'cli' : codexForkSource(fork),
		model_provider: 'openai',
		base_instructions: { text: text(random, around(random, 18_000)) },
		git: { commit_hash: random.hex(40), branch: 'main', repository_url: null },
	});
	if (fork !== undefined) {
		yield* codexCopiedHistory(session, fork);
	}
	while (true) {
		const turnId = random.uuid();
		const model = random.pick(CODEX_MODELS);
		yield codexLine(session, 'turn_context', {
			turn_id: turnId,
			cwd: session.cwd,
			approval_policy: 'on-request',
			sandbox_policy: { type: 'workspace-write' },
			model,
			effort: 'high',
			summary: 'auto',
			collaboration_mode: {
				mode: 'default',
				settings: { model, reasoning_effort: random.chance(0.2) ? null : 'high' },
			},
		});
		yield codexLine(session, 'event_msg', {
			type: 'task_started',
			turn_id: turnId,
			model_context_window: null,
		});
		yield codexLine(session, 'response_item', {
			type: 'message',
			role: 'user',
			content: [{ type: 'input_text', text: text(random, around(random, 1500)) }],
		});
		const responses = random.between(1, 8);
		for (let response = 0; response < responses; response += 1) {
			yield codexLine(session, 'response_item', {
				type: 'reasoning',
				id: `rs_${random.hex(32)}`,
				summary: [{ type: 'summary_text', text: text(random, around(random, 300)) }],
				encrypted_content: opaque(random, around(random, 4000)),
			});
			yield codexItemCompleted(session, turnId);
			const callId = `call_${random.hex(24)}`;
			yield codexLine(session, 'response_item', {
				type: 'custom_tool_call',
				status: 'completed',
				call_id: callId,
				name: 'apply_patch',
				input: text(random, around(random, 700)),
			});
			yield codexLine(session, 'response_item', {
				type: 'custom_tool_call_output',
				call_id: callId,
				output: text(random, around(random, 4800)),
			});
			const usage = codexUsage(random);
			session.total = addUsage(session.total, usage);
			yield codexLine(
				session,
				'token_usage_record',
				codexUsageRecordPayload(session, turnId, usage),
			);
			yield codexLine(session, 'event_msg', {
				type: 'token_count',
				info: {
					total_token_usage: session.total,
					last_token_usage: usage,
					last_usage_includes_reasoning: true,
					model_context_window: 272_000,
				},
				rate_limits: {
					limit_id: 'codex',
					limit_name: null,
					primary: { used_percent: random.int(100), window_minutes: 300, resets_at: 1_790_000_000 },
					secondary: null,
					credits: { has_credits: false, unlimited: false, balance: null },
					individual_limit: null,
					spend_control_reached: null,
					plan_type: 'pro',
					rate_limit_reached_type: null,
				},
			});
		}
		yield codexLine(session, 'response_item', {
			type: 'message',
			role: 'assistant',
			content: [{ type: 'output_text', text: text(random, around(random, 3000)) }],
			phase: 'final',
		});
		if (random.chance(CODEX_COMPACTION_CHANCE)) {
			const latest = codexUsageRecordPayload(session, turnId, codexUsage(random));
			yield codexLine(session, 'token_usage_record', latest);
			yield codexLine(session, 'compacted', {
				message: '',
				replacement_history: [
					{
						type: 'message',
						role: 'user',
						content: [{ type: 'input_text', text: text(random, around(random, 180_000)) }],
					},
				],
				window_number: 2,
				window_id: random.uuid(),
				previous_window_id: random.uuid(),
				first_window_id: random.uuid(),
				compaction_response_id: latest.response_id,
				latest_token_usage_record: latest,
			});
		}
		if (random.chance(0.08)) {
			yield codexLine(session, 'world_state', { snapshot: text(random, around(random, 26_000)) });
		}
		yield codexLine(session, 'event_msg', {
			type: 'task_complete',
			turn_id: turnId,
			last_agent_message: null,
		});
	}
}

type GrokSession = {
	cwd: string;
	sessionId: string;
	model: string;
	random: Random;
	clockMs: number;
	events: number;
	promptIndex: number;
};

function grokLine(session: GrokSession, method: string, update: Line, meta: Line = {}): Line {
	session.clockMs += 50 + session.random.int(20_000);
	session.events += 1;
	return {
		timestamp: Math.floor(session.clockMs / 1000),
		method,
		params: {
			sessionId: session.sessionId,
			update,
			_meta: {
				...meta,
				eventId: `${session.sessionId}-${session.events}`,
				agentTimestampMs: session.clockMs,
			},
		},
	};
}

function grokTurnMeta(session: GrokSession, updateType: string, turnStartMs: number): Line {
	return {
		totalTokens: session.random.between(5000, 200_000),
		requestTrimmedTokens: 0,
		promptId: `prompt-${session.promptIndex}`,
		streamStartMs: turnStartMs + 1000,
		turnStartMs,
		updateType,
	};
}

function byteArray(random: Random, length: number): number[] {
	return Array.from(Buffer.from(text(random, length)));
}

function grokToolOutput(random: Random): Line {
	switch (random.int(3)) {
		case 0: {
			const content = text(random, around(random, 3000));
			return {
				type: 'FileContent',
				FileContent: {
					content,
					content_concise: content,
					absolute_path: `/tmp/ccusage-fixture/grok/${random.hex(8)}.rs`,
					offset: null,
					raw_output: content,
					total_lines: random.between(10, 2000),
				},
			};
		}
		case 1:
			return {
				type: 'Bash',
				command: text(random, 80),
				current_dir: '/tmp/ccusage-fixture/grok',
				description: text(random, 40),
				exit_code: 0,
				output: byteArray(random, around(random, 1500)),
				output_file: null,
				output_for_prompt: null,
				signal: null,
				timed_out: false,
				total_bytes: random.int(4000),
				truncated: false,
			};
		default:
			return {
				type: 'GrepSearch',
				stdout: byteArray(random, around(random, 2500)),
				stderr: [],
				exit_code: 0,
				match_count: random.int(60),
				file_matches: Array.from({ length: random.between(1, 6) }, () => ({
					path: `src/${random.hex(6)}.rs`,
					matches: Array.from({ length: random.between(1, 8) }, () => ({
						line_number: random.int(2000),
						content: text(random, around(random, 60)),
					})),
				})),
			};
	}
}

function grokUsage(random: Random, model: string): Line {
	const inputTokens = random.between(20_000, 4_000_000);
	const usage = {
		inputTokens,
		outputTokens: random.between(100, 60_000),
		totalTokens: 0,
		cachedReadTokens: Math.floor(inputTokens * (0.6 + random.next() * 0.38)),
		cacheCreationTokens: 0,
		reasoningTokens: random.between(0, 40_000),
		modelCalls: random.between(1, 100),
		apiDurationMs: random.between(1000, 1_000_000),
		costUsdTicks: random.chance(0.9) ? random.between(1_000_000, 30_000_000_000) : 0,
	};
	usage.totalTokens = usage.inputTokens + usage.outputTokens;
	return { ...usage, modelUsage: { [model]: usage }, numTurns: usage.modelCalls };
}

function* grokSessionLines(session: GrokSession): Generator<Line> {
	const { random } = session;
	yield grokLine(session, '_x.ai/session/update', {
		sessionUpdate: 'hook_execution',
		event_name: 'session_start',
		runs: [{ name: text(random, 40), status: { status: 'success', elapsed_ms: random.int(60) } }],
	});
	while (true) {
		session.promptIndex += 1;
		const turnStartMs = session.clockMs;
		yield grokLine(session, 'session/update', {
			sessionUpdate: 'user_message_chunk',
			content: { type: 'text', text: text(random, around(random, 3000)) },
		});
		const toolFree = random.chance(session.promptIndex === 1 ? 0.8 : 0.5);
		const tools = toolFree ? 0 : random.between(1, 40);
		for (let tool = 0; tool < tools; tool += 1) {
			if (random.chance(0.2)) {
				yield grokLine(
					session,
					'session/update',
					{
						sessionUpdate: 'agent_thought_chunk',
						content: { type: 'text', text: text(random, around(random, 1400)) },
					},
					{ ...grokTurnMeta(session, 'AgentThoughtChunk', turnStartMs), chunkId: tool },
				);
			}
			const toolCallId = `call_${random.hex(38)}`;
			const xaiTool = {
				'x.ai/tool': {
					version: 1,
					name: 'run_command',
					kind: 'run_command',
					namespace: 'grok_build',
					label: 'Run Command',
					read_only: false,
				},
			};
			yield grokLine(
				session,
				'session/update',
				{
					sessionUpdate: 'tool_call',
					toolCallId,
					title: 'run_command',
					rawInput: { command: text(random, around(random, 200)) },
					_meta: xaiTool,
				},
				{
					...grokTurnMeta(session, 'ToolCall', turnStartMs),
					updateParams: { toolCallId, title: 'run_command', kind: 'Other', status: 'Pending' },
				},
			);
			const rawOutput = grokToolOutput(random);
			yield grokLine(
				session,
				'session/update',
				{
					sessionUpdate: 'tool_call_update',
					toolCallId,
					status: 'completed',
					content: [
						{
							type: 'content',
							content: { type: 'text', text: text(random, around(random, 1200)) },
						},
					],
					rawOutput,
				},
				{
					...grokTurnMeta(session, 'ToolCallUpdate', turnStartMs),
					updateParams: { toolCallId, status: 'completed' },
				},
			);
			yield grokLine(session, '_x.ai/session/update', {
				sessionUpdate: 'hook_execution',
				event_name: 'post_tool_use',
				runs: [
					{ name: text(random, 40), status: { status: 'success', elapsed_ms: random.int(60) } },
				],
			});
		}
		yield grokLine(session, 'session/update', {
			sessionUpdate: 'agent_message_chunk',
			content: { type: 'text', text: text(random, around(random, 900)) },
		});
		yield grokLine(session, '_x.ai/session/update', {
			sessionUpdate: 'turn_completed',
			prompt_id: `prompt-${session.promptIndex}`,
			stop_reason: 'end_turn',
			agent_result: text(random, 60),
			usage: grokUsage(random, session.model),
			elapsed_ms: session.clockMs - turnStartMs,
		});
	}
}

async function createEmptyDirectory(directory: string, flagName: string): Promise<void> {
	await mkdir(directory, { recursive: true });
	if ((await readdir(directory)).length > 0) {
		throw new Error(`${flagName} must be absent or empty: ${directory}`);
	}
}

function paddedNumber(value: number, width: number): string {
	return String(value).padStart(width, '0');
}

type PlannedFile = {
	path: string;
	startMs: number;
	lines: Generator<Line>;
	compressed?: boolean;
	sidecar?: { path: string; value: Line };
	keepLines?: number;
	trailer?: Line;
};

function claudeProxy(random: Random): ClaudeSession['proxy'] {
	switch (random.int(3)) {
		case 0:
			return { messageIdPrefix: 'msg_' };
		case 1:
			return { messageIdPrefix: 'chatcmpl-' };
		default:
			return { messageIdPrefix: '', constantMessageId: `gateway-${random.hex(8)}` };
	}
}

function planClaudeFile(
	directory: string,
	fileIndex: number,
	shape: ClaudeShape,
	previous?: ClaudeSession,
): {
	file: PlannedFile;
	session: ClaudeSession;
} {
	const random = new Random(1000 + fileIndex);
	const day = fileIndex % FIXTURE_DAYS;
	const startMs = FIXTURE_START_MS + day * DAY_MS + random.int(DAY_MS / 2);
	const burstSlot = fileIndex % SUBAGENT_BURST_EVERY;
	if (previous && burstSlot >= SUBAGENT_BURST_EVERY - SUBAGENT_BURST_FILES) {
		const agentId = random.hex(17);
		const session: ClaudeSession = {
			...previous,
			agentId,
			clock: new Clock(startMs),
			parentMessageIds: [...previous.parentMessageIds],
			random,
			lastUuid: undefined,
		};
		return {
			file: {
				path: join(
					directory,
					'projects',
					previous.projectDir,
					previous.sessionId,
					'subagents',
					`agent-${agentId}.jsonl`,
				),
				startMs,
				lines: claudeSessionLines(session),
			},
			session: previous,
		};
	}
	const projectCount =
		shape === 'many-small-files' ? CLAUDE_MANY_SMALL_FILES_PROJECT_COUNT : CLAUDE_PROJECT_COUNT;
	const project = `project-${paddedNumber(fileIndex % projectCount, 4)}`;
	const projectDir = `-tmp-ccusage-fixture-${project}`;
	const session: ClaudeSession = {
		clock: new Clock(startMs),
		cwd: `/tmp/ccusage-fixture/-tmp-ccusage-fixture-${project}/x`,
		projectDir,
		sessionId: random.uuid(),
		version: fileIndex % LEGACY_PROGRESS_FILE_EVERY === 0 ? '2.0.40' : '2.1.90',
		legacyProgress: fileIndex % LEGACY_PROGRESS_FILE_EVERY === 0,
		parentMessageIds: [],
		random,
		proxy: shape === 'requestless-proxy' ? claudeProxy(random) : undefined,
	};
	return {
		file: {
			path: join(directory, 'projects', projectDir, `${session.sessionId}.jsonl`),
			startMs,
			lines: claudeSessionLines(session),
			trailer:
				burstSlot === SUBAGENT_BURST_EVERY - SUBAGENT_BURST_FILES - 1
					? claudeFileHistoryUpdate(session, startMs, new Random(2_000_000 + fileIndex))
					: undefined,
		},
		session,
	};
}

// NOTE(2026-11-21, #2431): A file-history update repeats an earlier snapshot's time only inside `snapshot`, so a transcript ending in a large one has no record timestamp in its last 16 KiB. Its session's subagent transcripts make `--since` pay for any rule that revives them.
function claudeFileHistoryUpdate(session: ClaudeSession, startMs: number, random: Random): Line {
	const trackedFileBackups = Object.fromEntries(
		Array.from({ length: FILE_HISTORY_TRACKED_FILES }, (_, index) => [
			`${session.cwd}/src/${random.hex(6)}/${random.hex(8)}.ts`,
			{
				backupFileName: `${random.hex(16)}@v${(index % 5) + 1}`,
				version: (index % 5) + 1,
				backupTime: new Date(startMs + index * 1000).toISOString(),
			},
		]),
	);
	return {
		type: 'file-history-snapshot',
		messageId: random.uuid(),
		snapshot: {
			messageId: random.uuid(),
			trackedFileBackups,
			timestamp: new Date(startMs).toISOString(),
		},
		isSnapshotUpdate: true,
	};
}

function planCodexFile(
	directory: string,
	fileIndex: number,
	layout: CodexLayout,
	parent?: CodexParent,
): PlannedFile & { sessionId: string } {
	const random = new Random(500_000 + fileIndex);
	const startMs = FIXTURE_START_MS + (fileIndex % FIXTURE_DAYS) * DAY_MS + random.int(DAY_MS / 2);
	const start = new Date(startMs);
	const sessionId = random.uuid();
	const stamp = start.toISOString().slice(0, 19).replaceAll(':', '-');
	const session: CodexSession = {
		clock: new Clock(startMs),
		cwd: `/tmp/ccusage-fixture/codex-project-${paddedNumber(fileIndex % 24, 2)}`,
		sessionId,
		random,
		ordinal: 0,
		total: {},
		fork:
			parent !== undefined && fileIndex % CODEX_FORK_FILE_EVERY === CODEX_FORK_FILE_EVERY - 1
				? { parent, spawned: random.chance(0.5) }
				: undefined,
	};
	const name = `rollout-${stamp}-${sessionId}.jsonl`;
	const day = fileIndex % FIXTURE_DAYS;
	const compressed = layout === 'current' && day < FIXTURE_DAYS - CODEX_PLAIN_RECENT_DAYS;
	const archived = compressed && fileIndex % CODEX_ARCHIVED_FILE_EVERY === 0;
	return {
		path: archived
			? join(directory, 'archived_sessions', `${name}.zst`)
			: join(
					directory,
					'sessions',
					String(start.getUTCFullYear()),
					paddedNumber(start.getUTCMonth() + 1, 2),
					paddedNumber(start.getUTCDate(), 2),
					compressed ? `${name}.zst` : name,
				),
		startMs,
		lines: codexSessionLines(session),
		compressed,
		keepLines: CODEX_FORK_HISTORY_LINES,
		sessionId,
	};
}

function planGrokFile(directory: string, fileIndex: number): PlannedFile {
	const random = new Random(900_000 + fileIndex);
	const startMs = FIXTURE_START_MS + (fileIndex % FIXTURE_DAYS) * DAY_MS + random.int(DAY_MS / 2);
	const cwd = `/tmp/ccusage-fixture/grok-project-${paddedNumber(fileIndex % 12, 2)}`;
	const session: GrokSession = {
		cwd,
		sessionId: random.uuid(),
		model: random.pick(GROK_MODELS),
		random,
		clockMs: startMs,
		events: 0,
		promptIndex: 0,
	};
	const sessionDir = join(directory, 'sessions', encodeURIComponent(cwd), session.sessionId);
	return {
		path: join(sessionDir, 'updates.jsonl'),
		startMs,
		lines: grokSessionLines(session),
		sidecar: {
			path: join(sessionDir, 'summary.json'),
			value: {
				info: { id: session.sessionId, cwd },
				session_summary: text(random, 120),
				created_at: new Date(startMs).toISOString(),
				current_model_id: session.model.replace(/-build$/, ''),
				chat_format_version: 1,
				agent_name: 'grok-build',
			},
		},
	};
}

function lineTimeMs(line: Line): number | undefined {
	const { timestamp } = line;
	if (typeof timestamp === 'number') {
		return timestamp * 1000;
	}
	return typeof timestamp === 'string' ? Date.parse(timestamp) : undefined;
}

async function writePlannedFile(
	file: PlannedFile,
	targetSize: number,
): Promise<{ bytes: number; diskBytes: number; lines: number; kept: Line[] }> {
	await mkdir(dirname(file.path), { recursive: true });
	const chunks: string[] = [];
	const kept: Line[] = [];
	let bytes = 0;
	let lines = 0;
	let lastMs = file.startMs;
	while (bytes < targetSize) {
		const next = file.lines.next();
		if (next.done) {
			break;
		}
		const line = `${JSON.stringify(next.value)}\n`;
		const lineBytes = Buffer.byteLength(line);
		if (lines > 0 && bytes + lineBytes - targetSize > targetSize - bytes) {
			break;
		}
		chunks.push(line);
		bytes += lineBytes;
		lines += 1;
		lastMs = Math.max(lastMs, lineTimeMs(next.value) ?? lastMs);
		if (kept.length < (file.keepLines ?? 0)) {
			kept.push(next.value);
		}
	}
	if (file.trailer !== undefined) {
		const line = `${JSON.stringify(file.trailer)}\n`;
		chunks.push(line);
		bytes += Buffer.byteLength(line);
		lines += 1;
	}
	const content = Buffer.from(chunks.join(''));
	const data = file.compressed ? zstdCompressSync(content) : content;
	// NOTE(2026-11-20, #2340): Bounded Claude reports skip files by mtime. Date each file at its last record so `--since` prunes what real logs would.
	const mtime = new Date(lastMs);
	await writeFile(file.path, data);
	await utimes(file.path, mtime, mtime);
	if (file.sidecar) {
		await writeFile(file.sidecar.path, JSON.stringify(file.sidecar.value));
		await utimes(file.sidecar.path, mtime, mtime);
	}
	return { bytes, diskBytes: data.byteLength, lines, kept };
}

async function generateFixture(
	outputDir: string,
	sizeMib: number,
	format: Format,
	{
		codexLayout = 'current',
		claudeShape = 'default',
	}: Partial<Pick<Options, 'codexLayout' | 'claudeShape'>> = {},
): Promise<FixtureResult> {
	const fileSizeTargets = createFileSizeTargets(sizeProfile(format, claudeShape), sizeMib * MIB);

	let totalBytes = 0;
	let diskBytes = 0;
	let lineCount = 0;
	let previousSession: ClaudeSession | undefined;
	let codexParent: CodexParent | undefined;
	for (let fileIndex = 0; fileIndex < fileSizeTargets.length; fileIndex += 1) {
		const targetSize = fileSizeTargets[shuffledIndex(fileIndex, fileSizeTargets.length)];
		if (targetSize === undefined) {
			throw new Error(`Missing file size target for index ${fileIndex}`);
		}
		let file: PlannedFile;
		let codexSessionId: string | undefined;
		if (format === 'claude') {
			const planned = planClaudeFile(outputDir, fileIndex, claudeShape, previousSession);
			file = planned.file;
			previousSession = planned.session;
		} else if (format === 'codex') {
			const planned = planCodexFile(outputDir, fileIndex, codexLayout, codexParent);
			file = planned;
			codexSessionId = planned.sessionId;
		} else {
			file = planGrokFile(outputDir, fileIndex);
		}
		const written = await writePlannedFile(file, targetSize);
		if (codexSessionId !== undefined) {
			codexParent = { sessionId: codexSessionId, history: written.kept };
		}
		totalBytes += written.bytes;
		diskBytes += written.diskBytes;
		lineCount += written.lines;
	}

	return { fileCount: fileSizeTargets.length, lineCount, totalBytes, diskBytes };
}

const OPENCODE_MODELS = [
	{ id: 'claude-sonnet-4-5', providerID: 'anthropic' },
	{ id: 'claude-sonnet-4-5', providerID: 'anthropic' },
	{ id: 'claude-haiku-4-5', providerID: 'anthropic' },
	{ id: 'gpt-5.2', providerID: 'openai' },
] as const;

function openCodeContent(random: Random, bytes: number): Line[] {
	const parts: Line[] = [];
	for (let remaining = bytes; remaining > 0;) {
		const length = Math.min(remaining, around(random, 6000));
		parts.push(
			random.chance(0.5)
				? { type: 'text', text: text(random, length) }
				: {
						type: 'tool',
						tool: random.pick(['bash', 'read', 'edit', 'grep']),
						callID: `call_${random.hex(24)}`,
						state: { status: 'completed', output: text(random, length) },
					},
		);
		remaining -= length;
	}
	return parts;
}

function openCodeAssistant(random: Random, createdMs: number, contentBytes: number): Line {
	const input = random.between(1, 4000);
	const output = random.between(1, 4000);
	const reasoning = random.between(0, 2000);
	const cache = { read: random.between(0, 150_000), write: random.between(0, 20_000) };
	// NOTE(2026-11-21, #2407): OpenCode writes the message content before its token counts. Keep that order so reports scan the whole payload, as they do on real databases.
	return {
		role: 'assistant',
		content: openCodeContent(random, contentBytes),
		model: random.pick(OPENCODE_MODELS),
		time: { created: createdMs, completed: createdMs + random.between(500, 60_000) },
		tokens: { input, output, reasoning, cache, total: input + output + reasoning + cache.read },
		cost: random.between(1, 50_000) / 1_000_000,
		finish: 'stop',
	};
}

async function generateOpenCodeFixture(outputDir: string, sizeMib: number): Promise<FixtureResult> {
	const targetBytes = sizeMib * MIB;
	const random = new Random(23);
	const dbPath = join(outputDir, 'opencode.db');
	const db = new DatabaseSync(dbPath);
	db.exec(`PRAGMA journal_mode = WAL;
		PRAGMA synchronous = OFF;
		CREATE TABLE session_message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, type TEXT NOT NULL,
			seq INTEGER NOT NULL DEFAULT 0, time_created INTEGER NOT NULL, data TEXT NOT NULL);
		BEGIN;`);
	const insert = db.prepare(
		'INSERT INTO session_message (id, session_id, type, seq, time_created, data) VALUES (?, ?, ?, ?, ?, ?)',
	);
	let totalBytes = 0;
	let lineCount = 0;
	let sessionId = '';
	for (let row = 0; totalBytes < targetBytes; row += 1) {
		const seq = row % OPENCODE_SESSION_ROWS;
		if (seq === 0) {
			sessionId = `ses_${random.hex(26)}`;
		}
		const progress = totalBytes / targetBytes;
		const createdMs = Math.round(FIXTURE_START_MS + progress * FIXTURE_DAYS * DAY_MS);
		const assistant = row % 2 === 1;
		let contentBytes = assistant
			? around(random, 2000 + 45_000 * progress ** 2)
			: around(random, 800);
		let data = '';
		do {
			data = JSON.stringify(
				assistant
					? openCodeAssistant(random, createdMs, contentBytes)
					: {
							role: 'user',
							content: openCodeContent(random, contentBytes),
							time: { created: createdMs },
						},
			);
			contentBytes = Math.floor(contentBytes * 0.9);
		} while (Buffer.byteLength(data) > OPENCODE_MAX_PAYLOAD_BYTES);
		insert.run(
			`msg_${random.hex(26)}`,
			sessionId,
			assistant ? 'assistant' : 'user',
			seq,
			createdMs,
			data,
		);
		totalBytes += Buffer.byteLength(data);
		lineCount += 1;
	}
	db.exec(`COMMIT;
		CREATE INDEX session_message_time_created_idx ON session_message (time_created);`);
	db.close();
	return { fileCount: 1, lineCount, totalBytes, diskBytes: (await stat(dbPath)).size };
}

function printResult(name: string, outputDir: string, result: FixtureResult): void {
	console.log(`Generated ${name} fixture ${outputDir}`);
	console.log(`Files: ${result.fileCount}`);
	console.log(`Rows: ${result.lineCount}`);
	console.log(`Size on disk: ${formatBytes(result.diskBytes)}`);
	console.log(`Uncompressed: ${formatBytes(result.totalBytes)}`);
}

async function main(): Promise<void> {
	const options = parseArguments(process.argv.slice(2));
	const outputDir = resolve(options.outputDir);
	const codexOutputDir =
		options.codexOutputDir === undefined ? undefined : resolve(options.codexOutputDir);
	const grokOutputDir =
		options.grokOutputDir === undefined ? undefined : resolve(options.grokOutputDir);
	await createEmptyDirectory(outputDir, '--output-dir');
	if (codexOutputDir !== undefined) {
		await createEmptyDirectory(codexOutputDir, '--codex-output-dir');
	}

	if (grokOutputDir !== undefined) {
		await createEmptyDirectory(grokOutputDir, '--grok-output-dir');
	}
	const opencodeOutputDir =
		options.opencodeOutputDir === undefined ? undefined : resolve(options.opencodeOutputDir);
	if (opencodeOutputDir !== undefined) {
		await createEmptyDirectory(opencodeOutputDir, '--opencode-output-dir');
	}

	const claudeResult = await generateFixture(outputDir, options.sizeMib, 'claude', options);
	printResult('Claude', outputDir, claudeResult);

	if (codexOutputDir !== undefined) {
		const codexResult = await generateFixture(
			codexOutputDir,
			options.codexSizeMib,
			'codex',
			options,
		);
		printResult('Codex', codexOutputDir, codexResult);
	}

	if (grokOutputDir !== undefined) {
		const grokResult = await generateFixture(grokOutputDir, options.grokSizeMib, 'grok');
		printResult('Grok', grokOutputDir, grokResult);
	}

	if (opencodeOutputDir !== undefined) {
		const opencodeResult = await generateOpenCodeFixture(
			opencodeOutputDir,
			options.opencodeSizeMib,
		);
		printResult('OpenCode', opencodeOutputDir, opencodeResult);
	}
}

await main();
