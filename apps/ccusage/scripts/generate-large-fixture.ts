#!/usr/bin/env nix
/*
#! nix shell --inputs-from ../../.. nixpkgs#bun -c bun
*/
// Writes Claude Code and Codex logs whose line shapes, line-type mix, and
// per-type byte share follow aggregate statistics of real logs, with every
// value synthesized. Parser cost depends on those shapes (null-valued fields,
// nested usage, escaped strings, non-usage lines), so a fixture made of
// uniform padded usage lines hides regressions that real logs expose.
import { dirname, join, resolve } from 'node:path';

const MIB = 1024 * 1024;
const DAY_MS = 24 * 60 * 60 * 1000;
const FIXTURE_START_MS = Date.UTC(2026, 5, 1);
const FIXTURE_DAYS = 120;
const CLAUDE_PROJECT_COUNT = 48;
const SUBAGENT_FILE_EVERY = 4;
const LEGACY_PROGRESS_FILE_EVERY = 6;
const CODEX_PLAIN_RECENT_DAYS = 14;
const CODEX_ARCHIVED_FILE_EVERY = 4;
const CODEX_COMPACTION_CHANCE = 1 / 30;
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

const CLAUDE_MODELS = [
	'claude-opus-4-5-20251101',
	'claude-opus-4-5-20251101',
	'claude-sonnet-4-5-20250929',
	'claude-haiku-4-5-20251001',
] as const;
const CODEX_MODELS = [
	'gpt-5.2-codex',
	'gpt-5.2-codex',
	'gpt-5.3-codex',
	'gpt-5.1-codex-mini',
] as const;
const WORDS = [
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
	sizeMib: number;
	codexSizeMib: number;
	codexLayout: CodexLayout;
};

type Line = Record<string, unknown>;

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
		if (
			![
				'--output-dir',
				'--codex-output-dir',
				'--size-mib',
				'--codex-size-mib',
				'--codex-layout',
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

	return {
		outputDir,
		codexOutputDir: values.get('--codex-output-dir'),
		sizeMib: parsePositiveInteger(values.get('--size-mib') ?? '1024', '--size-mib'),
		codexSizeMib: parsePositiveInteger(
			values.get('--codex-size-mib') ?? '1024',
			'--codex-size-mib',
		),
		codexLayout,
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

/** Deterministic PRNG (mulberry32) so every run writes byte-identical fixtures. */
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

function buildTextPool(): string {
	const random = new Random(7);
	const parts: string[] = [];
	let length = 0;
	while (length < 2 * MIB) {
		const word = random.chance(0.08) ? '\n' : random.pick(WORDS);
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

const TEXT_POOL = buildTextPool();
const OPAQUE_POOL = buildOpaquePool();

// Pool slices stay within the Basic Multilingual Plane, so a slice boundary can
// never split a surrogate pair into an invalid JSON escape.
function text(random: Random, length: number): string {
	const start = random.int(TEXT_POOL.length - length - 1);
	return TEXT_POOL.slice(start, start + length);
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
};

function claudeEnvelope(session: ClaudeSession, type: string, timestamp: string): Line {
	return {
		parentUuid: session.random.uuid(),
		isSidechain: session.agentId !== undefined,
		...(session.agentId === undefined ? {} : { agentId: session.agentId }),
		userType: 'external',
		entrypoint: 'cli',
		cwd: session.cwd,
		sessionId: session.sessionId,
		version: session.version,
		gitBranch: 'main',
		type,
		uuid: session.random.uuid(),
		timestamp,
	};
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
				thinking: text(random, around(random, 1600)),
				signature: opaque(random, around(random, 600)),
			};
		case 'tool_use':
			return {
				type: 'tool_use',
				id: `toolu_${random.hex(24)}`,
				name: random.pick(['Bash', 'Read', 'Edit', 'Grep', 'Task']),
				input: { command: text(random, around(random, 900)), description: text(random, 60) },
				caller: { type: 'direct' },
			};
		default:
			return { type: 'text', text: text(random, around(random, 1800)) };
	}
}

/** One API response, written by Claude Code as one line per content block that all repeat the usage. */
function claudeAssistantResponse(session: ClaudeSession, messageId?: string): Line[] {
	const { random } = session;
	const model = random.chance(0.01) ? '<synthetic>' : random.pick(CLAUDE_MODELS);
	const id = messageId ?? `msg_${random.hex(24)}`;
	const requestId = `req_${random.hex(24)}`;
	const usage = claudeUsage(random, model);
	const blocks = random.chance(0.6) ? ['thinking'] : [];
	blocks.push(random.chance(0.55) ? 'tool_use' : 'text');
	if (random.chance(0.3)) {
		blocks.push('tool_use');
	}
	const timestamp = session.clock.tick(random);
	return blocks.map((kind) => {
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
		if (session.legacyProgress && session.agentId === undefined && random.chance(0.25)) {
			return {
				...claudeEnvelope(session, 'progress', timestamp),
				data: {
					message: { type: 'assistant', message, requestId, uuid: random.uuid(), timestamp },
					normalizedMessages: [],
					type: 'agent_progress',
					agentId: random.hex(17),
				},
				toolUseID: `toolu_${random.hex(24)}`,
				parentToolUseID: `toolu_${random.hex(24)}`,
			};
		}
		return {
			...claudeEnvelope(session, 'assistant', timestamp),
			message,
			requestId,
			requestedModel: model,
			effort: 'high',
			apiBlockIndex: 0,
			promptCacheRequested: true,
			advisorModel: null,
		};
	});
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
	// A sidechain transcript replays parent messages under new request IDs; the
	// loaders must drop those copies, so the fixture exercises that path.
	for (const messageId of session.parentMessageIds.slice(0, 2)) {
		yield* claudeAssistantResponse(session, messageId);
	}
	while (true) {
		yield claudeUser(session, false);
		const steps = random.between(1, 6);
		for (let step = 0; step < steps; step += 1) {
			yield* claudeSideLines(session);
			const response = claudeAssistantResponse(session);
			const firstMessage = response[0]?.message as { id?: string } | undefined;
			if (
				session.agentId === undefined &&
				firstMessage?.id &&
				session.parentMessageIds.length < 4
			) {
				session.parentMessageIds.push(firstMessage.id);
			}
			yield* response;
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
};

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

function codexUsageRecord(session: CodexSession, turnId: string, usage: Line): Line {
	return codexLine(session, 'token_usage_record', {
		thread_id: session.sessionId,
		turn_id: turnId,
		session_id: session.sessionId,
		root_turn_id: turnId,
		response_id: `resp_${session.random.hex(32)}`,
		usage,
		turn_token_usage: usage,
		thread_token_usage: session.total,
	});
}

function* codexSessionLines(session: CodexSession): Generator<Line> {
	const { random } = session;
	yield codexLine(session, 'session_meta', {
		session_id: session.sessionId,
		id: session.sessionId,
		timestamp: new Date(FIXTURE_START_MS).toISOString(),
		cwd: session.cwd,
		originator: 'codex_cli_rs',
		cli_version: '0.200.0',
		source: 'cli',
		model_provider: 'openai',
		base_instructions: { text: text(random, around(random, 18_000)) },
		git: { commit_hash: random.hex(40), branch: 'main', repository_url: null },
	});
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
			// Codex writes the response's usage record before the token_count
			// that advances the cumulative total, which is what lets the parser
			// settle each record instead of keeping it pending.
			yield codexUsageRecord(session, turnId, usage);
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
			// A remote compaction's usage is missing from the cumulative total,
			// so its record stays pending until the marker naming it arrives.
			const record = codexUsageRecord(session, turnId, codexUsage(random));
			yield record;
			const latest = record.payload as Line;
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

function assertSafeDeletionTarget(directory: string, flagName: string): string {
	const resolved = resolve(directory);
	if (resolved === resolve('/') || resolved === resolve('.') || resolved.length < 5) {
		throw new Error(`Refusing to delete unsafe ${flagName} path: ${resolved}`);
	}
	return resolved;
}

function paddedNumber(value: number, width: number): string {
	return String(value).padStart(width, '0');
}

type PlannedFile = { path: string; lines: Generator<Line>; compressed?: boolean };

function planClaudeFile(
	directory: string,
	fileIndex: number,
	previous?: ClaudeSession,
): {
	file: PlannedFile;
	session: ClaudeSession;
} {
	const random = new Random(1000 + fileIndex);
	const day = fileIndex % FIXTURE_DAYS;
	const startMs = FIXTURE_START_MS + day * DAY_MS + random.int(DAY_MS / 2);
	if (previous && fileIndex % SUBAGENT_FILE_EVERY === SUBAGENT_FILE_EVERY - 1) {
		const agentId = random.hex(17);
		const session: ClaudeSession = {
			...previous,
			agentId,
			clock: new Clock(startMs),
			parentMessageIds: [...previous.parentMessageIds],
			random,
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
				lines: claudeSessionLines(session),
			},
			session: previous,
		};
	}
	const project = `project-${paddedNumber(fileIndex % CLAUDE_PROJECT_COUNT, 3)}`;
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
	};
	return {
		file: {
			path: join(directory, 'projects', projectDir, `${session.sessionId}.jsonl`),
			lines: claudeSessionLines(session),
		},
		session,
	};
}

function planCodexFile(directory: string, fileIndex: number, layout: CodexLayout): PlannedFile {
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
	};
	const name = `rollout-${stamp}-${sessionId}.jsonl`;
	const day = fileIndex % FIXTURE_DAYS;
	// Codex compresses rollouts once they age and moves some to
	// archived_sessions, so most real Codex bytes are read through zstd. The
	// legacy layout keeps every rollout plain under sessions/, which ccusage
	// releases before Zstandard (#1890) and archived-session (#1230) support
	// read in full, so they can be compared on the same input.
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
		lines: codexSessionLines(session),
		compressed,
	};
}

async function writeFile(
	file: PlannedFile,
	targetSize: number,
): Promise<{ bytes: number; diskBytes: number; lines: number }> {
	await Bun.$`mkdir -p ${dirname(file.path)}`;
	const chunks: string[] = [];
	let bytes = 0;
	let lines = 0;
	while (bytes < targetSize) {
		const next = file.lines.next();
		if (next.done) {
			break;
		}
		const line = `${JSON.stringify(next.value)}\n`;
		chunks.push(line);
		bytes += Buffer.byteLength(line);
		lines += 1;
	}
	const content = chunks.join('');
	const diskBytes = await Bun.write(
		file.path,
		file.compressed ? Bun.zstdCompressSync(content) : content,
	);
	return { bytes, diskBytes, lines };
}

async function generateFixture(
	outputDir: string,
	sizeMib: number,
	format: 'claude' | 'codex',
	codexLayout: CodexLayout = 'current',
): Promise<FixtureResult> {
	const directory = assertSafeDeletionTarget(
		outputDir,
		format === 'claude' ? '--output-dir' : '--codex-output-dir',
	);
	const fileSizeTargets = createFileSizeTargets(sizeMib * MIB);
	await Bun.$`rm -rf ${directory}`;

	let totalBytes = 0;
	let diskBytes = 0;
	let lineCount = 0;
	let previousSession: ClaudeSession | undefined;
	for (let fileIndex = 0; fileIndex < fileSizeTargets.length; fileIndex += 1) {
		const targetSize = fileSizeTargets[shuffledIndex(fileIndex, fileSizeTargets.length)];
		if (targetSize === undefined) {
			throw new Error(`Missing file size target for index ${fileIndex}`);
		}
		let file: PlannedFile;
		if (format === 'claude') {
			const planned = planClaudeFile(directory, fileIndex, previousSession);
			file = planned.file;
			previousSession = planned.session;
		} else {
			file = planCodexFile(directory, fileIndex, codexLayout);
		}
		const written = await writeFile(file, targetSize);
		totalBytes += written.bytes;
		diskBytes += written.diskBytes;
		lineCount += written.lines;
	}

	return { fileCount: fileSizeTargets.length, lineCount, totalBytes, diskBytes };
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
	const claudeResult = await generateFixture(outputDir, options.sizeMib, 'claude');
	printResult('Claude', outputDir, claudeResult);

	if (options.codexOutputDir) {
		const codexOutputDir = resolve(options.codexOutputDir);
		const codexResult = await generateFixture(
			codexOutputDir,
			options.codexSizeMib,
			'codex',
			options.codexLayout,
		);
		printResult('Codex', codexOutputDir, codexResult);
	}
}

await main();
