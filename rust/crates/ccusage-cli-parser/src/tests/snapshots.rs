use super::*;

fn shared_snapshot(shared: &SharedArgs) -> Value {
    json!({
        "since": shared.since.map(|date| date.to_string()).as_deref(),
        "until": shared.until.map(|date| date.to_string()).as_deref(),
        "json": shared.json,
        "mode": format!("{:?}", shared.mode),
        "debug": shared.debug,
        "debugSamples": shared.debug_samples,
        "order": format!("{:?}", shared.order.unwrap_or_default()),
        "breakdown": shared.breakdown,
        "offline": shared.offline,
        "color": format!("{:?}", shared.color),
        "timezone": shared.timezone.as_ref().map(|timezone| timezone.name()),
        "jq": shared.jq.as_deref(),
        "compact": shared.compact,
        "singleThread": shared.single_thread,
    })
}

fn cli_snapshot(cli: Command) -> Value {
    json!({
        "command": command_snapshot(cli),
    })
}

fn command_snapshot(command: Command) -> Value {
    match command {
        Command::All(args) => agent_command_snapshot("all", args),
        Command::Claude {
            report: ClaudeReport::Daily(args),
            project,
        } => json!({
            "type": "daily",
            "shared": shared_snapshot(&args.shared),
            "instances": args.instances,
            "project": project,
            "projectAliases": args.project_aliases,
        }),
        Command::Claude {
            report: ClaudeReport::Monthly(shared),
            ..
        } => json!({
            "type": "monthly",
            "shared": shared_snapshot(&shared),
        }),
        Command::Claude {
            report: ClaudeReport::Weekly(args),
            ..
        } => json!({
            "type": "weekly",
            "shared": shared_snapshot(&args.shared),
            "startOfWeek": format!("{:?}", args.start_of_week),
        }),
        Command::Claude {
            report: ClaudeReport::Session(args),
            ..
        } => json!({
            "type": "session",
            "shared": shared_snapshot(&args.shared),
            "id": args.id,
        }),
        Command::Claude {
            report: ClaudeReport::Blocks(args),
            ..
        } => json!({
            "type": "blocks",
            "shared": shared_snapshot(&args.shared),
            "active": args.active,
            "recent": args.recent,
            "tokenLimit": args.token_limit.map(|value| value.to_string()),
            "sessionLength": args.session_length.as_millis() as f64 / 3_600_000.0,
        }),
        Command::Statusline(args) => json!({
            "type": "statusline",
            "offline": args.offline,
            "visualBurnRate": format!("{:?}", args.visual_burn_rate),
            "costSource": format!("{:?}", args.cost_source),
            "cache": args.cache,
            "refreshInterval": args.refresh_interval,
            "contextLowThreshold": args.context_thresholds.low(),
            "contextMediumThreshold": args.context_thresholds.medium(),
            "debug": args.debug,
        }),
        Command::Telemetry(action) => json!({
            "type": "telemetry",
            "action": format!("{action:?}"),
        }),
        Command::Agent(_, AgentCommand::Statusline(args)) => json!({
            "type": "codex-statusline",
            "shared": shared_snapshot(&args.shared),
            "sessionId": args.session_id,
            "speed": format!("{:?}", args.codex_speed),
            "mcp": args.mcp,
        }),
        Command::Agent(agent, AgentCommand::Report(args)) => {
            agent_command_snapshot(agent.spec().name, args)
        }
    }
}

fn agent_command_snapshot(agent: &str, args: AgentCommandArgs) -> Value {
    json!({
        "type": agent,
        "shared": shared_snapshot(&args.shared),
        "kind": format!("{:?}", args.kind),
        "sections": args.sections.map(|sections| sections
            .into_iter()
            .map(|section| format!("{section:?}"))
            .collect::<Vec<_>>()),
        "byAgent": args.by_agent,
        "piPath": args.options.pi_path,
        "openClawPath": args.options.open_claw_path,
        "codexSpeed": format!("{:?}", args.options.codex_speed),
    })
}

#[test]
fn snapshots_representative_cli_parse_shapes() {
    let cases = vec![
        json!({
            "case": "default all-agent daily",
            "cli": cli_snapshot(parse(&["ccusage"])),
        }),
        json!({
            "case": "root daily with shared flags",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "--json",
                "--since=20260102",
                "--until",
                "20260110",
                "--mode",
                "calculate",
                "--debug",
                "--debug-samples",
                "9",
                "--order",
                "desc",
                "--breakdown",
                "--offline",
                "--no-offline",
                "--color",
                "--no-color",
                "--timezone",
                "Asia/Tokyo",
                "--jq",
                ".totals",
                "--compact",
                "--single-thread",
                "daily",
            ])),
        }),
        json!({
            "case": "claude weekly monday",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "claude",
                "weekly",
                "--start-of-week",
                "monday",
            ])),
        }),
        json!({
            "case": "claude daily project instances",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "claude",
                "daily",
                "--instances",
                "--project",
                "repo",
                "--project-aliases",
                "repo=Repository",
            ])),
        }),
        json!({
            "case": "codex monthly fast",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "codex",
                "monthly",
                "--speed=fast",
            ])),
        }),
        json!({
            "case": "opencode weekly",
            "cli": cli_snapshot(parse(&["ccusage", "opencode", "weekly", "--json"])),
        }),
        json!({
            "case": "pi session path",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "pi",
                "session",
                "--pi-path",
                "/tmp/pi-sessions",
            ])),
        }),
        json!({
            "case": "openclaw session path",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "openclaw",
                "session",
                "--open-claw-path=/tmp/openclaw",
            ])),
        }),
        json!({
            "case": "grok daily",
            "cli": cli_snapshot(parse(&["ccusage", "grok", "daily", "--json"])),
        }),
        json!({
            "case": "antigravity session",
            "cli": cli_snapshot(parse(&["ccusage", "antigravity", "session", "--json"])),
        }),
        json!({
            "case": "zcode daily",
            "cli": cli_snapshot(parse(&["ccusage", "zcode", "daily", "--json"])),
        }),
        json!({
            "case": "cline weekly",
            "cli": cli_snapshot(parse(&["ccusage", "cline", "weekly", "--json"])),
        }),
        json!({
            "case": "dsh weekly",
            "cli": cli_snapshot(parse(&["ccusage", "dsh", "weekly", "--json"])),
        }),
        json!({
            "case": "blocks active recent",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "blocks",
                "--active",
                "--recent",
                "--token-limit",
                "max",
                "--session-length=6.5",
            ])),
        }),
        json!({
            "case": "statusline thresholds",
            "cli": cli_snapshot(parse(&[
                "ccusage",
                "statusline",
                "--no-offline",
                "--visual-burn-rate",
                "emoji-text",
                "--cost-source",
                "both",
                "--no-cache",
                "--refresh-interval",
                "3",
                "--context-low-threshold",
                "45",
                "--context-medium-threshold",
                "75",
                "--debug",
            ])),
        }),
    ];

    insta::assert_json_snapshot!(cases);
}

#[test]
fn snapshots_cli_parse_error_guidance() {
    let cases = vec![
        json!({
            "args": ["ccusage", "--daily"],
            "error": parse_error(&["ccusage", "--daily"]),
        }),
        json!({
            "args": ["ccusage", "daily", "--agent", "codex"],
            "error": parse_error(&["ccusage", "daily", "--agent", "codex"]),
        }),
        json!({
            "args": ["ccusage", "codex", "blocks"],
            "error": parse_error(&["ccusage", "codex", "blocks"]),
        }),
        json!({
            "args": ["ccusage", "--mode", "bad"],
            "error": parse_error(&["ccusage", "--mode", "bad"]),
        }),
        json!({
            "args": ["ccusage", "blocks", "--session-length", "abc"],
            "error": parse_error(&["ccusage", "blocks", "--session-length", "abc"]),
        }),
        json!({
            "args": ["ccusage", "statusline", "--visual-burn-rate", "loud"],
            "error": parse_error(&[
                "ccusage",
                "statusline",
                "--visual-burn-rate",
                "loud",
            ]),
        }),
        json!({
            "args": ["ccusage", "codex", "yearly"],
            "error": parse_error(&["ccusage", "codex", "yearly"]),
        }),
    ];

    insta::assert_json_snapshot!(cases);
}
