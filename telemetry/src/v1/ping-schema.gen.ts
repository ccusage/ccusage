// Generated from `rust/crates/ccusage-telemetry` by `just telemetry::generate-schema`.
// Do not edit; change the Rust `Ping` type instead.
export const pingJsonSchema = {
	$schema: 'http://json-schema.org/draft-07/schema#',
	title: 'Ping',
	description:
		'The one document the CLI sends, at most once per UTC day.\n\nThe collector strips unknown keys rather than rejecting them, so a field can be added here before the Worker is redeployed; removing or tightening one breaks older CLIs that are still sending it.',
	type: 'object',
	required: ['anonymousId', 'arch', 'command', 'event', 'os', 'version'],
	properties: {
		anonymousId: {
			description: 'Random 128-bit value as lowercase hex, generated once per install.',
			type: 'string',
			pattern: '^[0-9a-f]{32}$',
		},
		arch: {
			type: 'string',
			pattern: '^[\\w .+-]{1,64}$',
		},
		command: {
			description: 'The command as typed, without arguments or option values.',
			type: 'string',
			pattern: '^[\\w .+-]{1,64}$',
		},
		event: {
			type: 'string',
			enum: ['cli_daily_active'],
		},
		os: {
			type: 'string',
			pattern: '^[\\w .+-]{1,64}$',
		},
		version: {
			type: 'string',
			pattern: '^[\\w .+-]{1,64}$',
		},
	},
} as const;
