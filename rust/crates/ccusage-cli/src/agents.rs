use crate::AgentReportKind;

#[derive(Debug)]
pub struct AgentSpec {
    id: AgentId,
    pub name: &'static str,
    pub label: &'static str,
    pub report_label: &'static str,
    pub reports: &'static [AgentReportKind],
    pub extra_commands: &'static [AgentCommandSpec],
}

#[derive(Debug)]
pub struct AgentCommandSpec {
    pub name: &'static str,
    pub kind: AgentCommandKind,
}

#[derive(Clone, Copy, Debug)]
pub enum AgentCommandKind {
    Blocks(&'static [(&'static str, BlocksOption)]),
    ClaudeStatusline {
        offline_by_default: bool,
        options: &'static [(&'static str, ClaudeStatuslineOption)],
    },
    CodexStatusline {
        offline_by_default: bool,
        options: &'static [(&'static str, CodexStatuslineOption)],
    },
}

impl AgentCommandSpec {
    pub(crate) const fn offline_by_default(&self) -> bool {
        match self.kind {
            AgentCommandKind::Blocks(_) => false,
            AgentCommandKind::ClaudeStatusline {
                offline_by_default, ..
            }
            | AgentCommandKind::CodexStatusline {
                offline_by_default, ..
            } => offline_by_default,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum BlocksOption {
    Active,
    Recent,
    TokenLimit,
    SessionLength,
}

#[derive(Clone, Copy, Debug)]
pub enum ClaudeStatuslineOption {
    Offline,
    NoOffline,
    VisualBurnRate,
    CostSource,
    Cache,
    NoCache,
    RefreshInterval,
    ContextLowThreshold,
    ContextMediumThreshold,
    Timezone,
    Config,
    Debug,
}

#[derive(Clone, Copy, Debug)]
pub enum CodexStatuslineOption {
    Id,
    Speed,
    Mcp,
}

const CLAUDE_BLOCKS: AgentCommandSpec = AgentCommandSpec {
    name: "blocks",
    kind: AgentCommandKind::Blocks(&[
        ("-a", BlocksOption::Active),
        ("--active", BlocksOption::Active),
        ("-r", BlocksOption::Recent),
        ("--recent", BlocksOption::Recent),
        ("-t", BlocksOption::TokenLimit),
        ("--token-limit", BlocksOption::TokenLimit),
        ("-n", BlocksOption::SessionLength),
        ("--session-length", BlocksOption::SessionLength),
    ]),
};

pub(crate) const CLAUDE_STATUSLINE: AgentCommandSpec = AgentCommandSpec {
    name: "statusline",
    kind: AgentCommandKind::ClaudeStatusline {
        offline_by_default: true,
        options: &[
            ("-O", ClaudeStatuslineOption::Offline),
            ("--offline", ClaudeStatuslineOption::Offline),
            ("--no-offline", ClaudeStatuslineOption::NoOffline),
            ("-B", ClaudeStatuslineOption::VisualBurnRate),
            ("--visual-burn-rate", ClaudeStatuslineOption::VisualBurnRate),
            ("--cost-source", ClaudeStatuslineOption::CostSource),
            ("--cache", ClaudeStatuslineOption::Cache),
            ("--no-cache", ClaudeStatuslineOption::NoCache),
            (
                "--refresh-interval",
                ClaudeStatuslineOption::RefreshInterval,
            ),
            (
                "--context-low-threshold",
                ClaudeStatuslineOption::ContextLowThreshold,
            ),
            (
                "--context-medium-threshold",
                ClaudeStatuslineOption::ContextMediumThreshold,
            ),
            ("-z", ClaudeStatuslineOption::Timezone),
            ("--timezone", ClaudeStatuslineOption::Timezone),
            ("--config", ClaudeStatuslineOption::Config),
            ("-d", ClaudeStatuslineOption::Debug),
            ("--debug", ClaudeStatuslineOption::Debug),
        ],
    },
};

const CODEX_STATUSLINE: AgentCommandSpec = AgentCommandSpec {
    name: "statusline",
    kind: AgentCommandKind::CodexStatusline {
        offline_by_default: true,
        options: &[
            ("-i", CodexStatuslineOption::Id),
            ("--id", CodexStatuslineOption::Id),
            ("--speed", CodexStatuslineOption::Speed),
            ("--mcp", CodexStatuslineOption::Mcp),
        ],
    },
};

impl AgentSpec {
    pub fn extra_command(&self, name: &str) -> Option<&AgentCommandSpec> {
        self.extra_commands
            .iter()
            .find(|command| command.name == name)
    }

    pub fn accepts(&self, command: &str) -> bool {
        self.extra_command(command).is_some()
            || AgentReportKind::from_name(command).is_some_and(|kind| self.reports.contains(&kind))
    }
}

const STANDARD_REPORTS: &[AgentReportKind] = &[
    AgentReportKind::Daily,
    AgentReportKind::Monthly,
    AgentReportKind::Session,
];

const ALL_REPORTS: &[AgentReportKind] = &[
    AgentReportKind::Daily,
    AgentReportKind::Weekly,
    AgentReportKind::Monthly,
    AgentReportKind::Session,
];

macro_rules! agents {
    (@report_label $label:literal) => { $label };
    (@report_label $label:literal, $report_label:literal) => { $report_label };
    (
        $own:ident => $own_name:literal, $own_label:literal $(/ $own_report_label:literal)?, $own_reports:expr, $own_extra:expr;
        $($id:ident => $name:literal, $label:literal $(/ $report_label:literal)?, $reports:expr, $extra:expr;)*
    ) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum AgentId {
            $own,
            $($id,)*
        }

        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum StandardAgent {
            $($id,)*
        }

        impl StandardAgent {
            pub const fn id(self) -> AgentId {
                match self {
                    $(Self::$id => AgentId::$id,)*
                }
            }
        }

        impl AgentId {
            pub const fn standard(self) -> Option<StandardAgent> {
                match self {
                    Self::$own => None,
                    $(Self::$id => Some(StandardAgent::$id),)*
                }
            }
        }

        pub const AGENTS: &[AgentSpec] = &[
            AgentSpec {
                id: AgentId::$own,
                name: $own_name,
                label: $own_label,
                report_label: agents!(@report_label $own_label $(, $own_report_label)?),
                reports: $own_reports,
                extra_commands: $own_extra,
            },
            $(AgentSpec {
                id: AgentId::$id,
                name: $name,
                label: $label,
                report_label: agents!(@report_label $label $(, $report_label)?),
                reports: $reports,
                extra_commands: $extra,
            },)*
        ];

        pub const BUILT_IN_AGENT_NAMES: &[&str] = &[$own_name, $($name,)*];
    };
}

agents! {
    Claude => "claude", "Claude Code" / "Claude", ALL_REPORTS, &[CLAUDE_BLOCKS, CLAUDE_STATUSLINE];
    Codex => "codex", "Codex", STANDARD_REPORTS, &[CODEX_STATUSLINE];
    OpenCode => "opencode", "OpenCode", ALL_REPORTS, &[];
    Amp => "amp", "Amp", STANDARD_REPORTS, &[];
    Droid => "droid", "Droid", STANDARD_REPORTS, &[];
    Codebuff => "codebuff", "Codebuff", STANDARD_REPORTS, &[];
    Hermes => "hermes", "Hermes", STANDARD_REPORTS, &[];
    Pi => "pi", "pi-agent", STANDARD_REPORTS, &[];
    Goose => "goose", "Goose", STANDARD_REPORTS, &[];
    OpenClaw => "openclaw", "OpenClaw", STANDARD_REPORTS, &[];
    Kilo => "kilo", "Kilo", STANDARD_REPORTS, &[];
    Copilot => "copilot", "GitHub Copilot CLI", ALL_REPORTS, &[];
    Gemini => "gemini", "Gemini CLI", STANDARD_REPORTS, &[];
    Antigravity => "antigravity", "Antigravity", ALL_REPORTS, &[];
    Kimi => "kimi", "Kimi", ALL_REPORTS, &[];
    Qwen => "qwen", "Qwen", STANDARD_REPORTS, &[];
    Grok => "grok", "Grok", STANDARD_REPORTS, &[];
    ZCode => "zcode", "ZCode", ALL_REPORTS, &[];
    Cline => "cline", "Cline", ALL_REPORTS, &[];
    Dsh => "dsh", "DeepSeek Harness", ALL_REPORTS, &[];
}

impl AgentId {
    pub fn from_colon_command(command: &str) -> Option<(Self, &str)> {
        let (name, report) = command.split_once(':')?;
        let agent = Self::from_name(name)?;
        agent.spec().accepts(report).then_some((agent, report))
    }

    pub const fn spec(self) -> &'static AgentSpec {
        &AGENTS[self as usize]
    }

    pub fn from_name(name: &str) -> Option<Self> {
        AGENTS
            .iter()
            .find(|spec| spec.name == name)
            .map(|spec| spec.id)
    }
}

impl StandardAgent {
    pub const fn spec(self) -> &'static AgentSpec {
        self.id().spec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_spec_sits_at_its_id() {
        for spec in AGENTS {
            assert_eq!(spec.id.spec().name, spec.name);
            assert_eq!(AgentId::from_name(spec.name), Some(spec.id));
        }
    }

    #[test]
    fn every_agent_but_claude_takes_the_standard_commands() {
        for spec in AGENTS {
            match spec.id.standard() {
                Some(agent) => assert_eq!(agent.id(), spec.id),
                None => assert_eq!(spec.id, AgentId::Claude),
            }
        }
    }

    #[test]
    fn accepts_reports_and_extra_commands_only() {
        let claude = AgentId::Claude.spec();
        assert!(claude.accepts("weekly"));
        assert!(claude.accepts("blocks"));
        let codex = AgentId::Codex.spec();
        assert!(codex.accepts("session"));
        assert!(!codex.accepts("weekly"));
        assert!(!codex.accepts("blocks"));
        assert!(!codex.accepts("yearly"));
    }

    #[test]
    fn recognizes_only_supported_colon_commands() {
        assert_eq!(
            AgentId::from_colon_command("codex:daily"),
            Some((AgentId::Codex, "daily"))
        );
        assert_eq!(
            AgentId::from_colon_command("claude:statusline"),
            Some((AgentId::Claude, "statusline"))
        );
        assert_eq!(AgentId::from_colon_command("codex:weekly"), None);
        assert_eq!(AgentId::from_colon_command("unknown:daily"), None);
    }
}
