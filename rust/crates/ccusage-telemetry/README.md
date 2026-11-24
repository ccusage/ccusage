# ccusage-telemetry

The wire format of the anonymous daily usage ping, shared by the CLI that sends
it and the collector Worker in `apps/telemetry` that receives it.

## Owns

- `lib.rs` — the `Ping` type, the single definition of the payload, and
  `generate_schema_module`, which renders its JSON Schema as a TypeScript module.
- `bin/generate_telemetry_schema.rs` — writes
  `apps/telemetry/src/v1/ping-schema.gen.ts`, which the Worker validates against
  and derives its TypeScript type from. Run it through
  `just telemetry::generate-schema`, or let `nix fmt` regenerate it whenever this
  crate changes.

The rest of the client — when to send, opt-outs, the stored state — stays in the
binary's `telemetry.rs`, since only the payload has to be shared.

## Public surface

- `Ping`
- `PingEvent`
- `generate_schema_module`

## Depends on

- `schemars`
- `serde`
- `serde_json`

## Build layer

Outside the Crane artifact layers: only the binary depends on it, so it is
compiled with the final binary and editing it leaves the cached layers untouched.
