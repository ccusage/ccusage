use crate::AgentReportKind;

#[derive(Debug)]
pub struct AgentSpec {
    pub id: AgentId,
    pub name: &'static str,
    pub label: &'static str,
    pub report_label: &'static str,
    pub reports: &'static [AgentReportKind],
    pub extra_commands: &'static [&'static str],
}

impl AgentSpec {
    pub fn accepts(&self, command: &str) -> bool {
        self.extra_commands.contains(&command)
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

        /// Agent subcommands that ship in the binary, in report load order.
        pub const BUILT_IN_AGENT_NAMES: &[&str] = &[$own_name, $($name,)*];
    };
}

agents! {
    Claude => "claude", "Claude Code" / "Claude", ALL_REPORTS, &["blocks", "statusline"];
    Codex => "codex", "Codex", STANDARD_REPORTS, &["statusline"];
    OpenCode => "opencode", "OpenCode", ALL_REPORTS, &[];
    Amp => "amp", "Amp", STANDARD_REPORTS, &[];
    Droid => "droid", "Droid", STANDARD_REPORTS, &[];
    Codebuff => "codebuff", "Codebuff", STANDARD_REPORTS, &[];
    Hermes => "hermes", "Hermes", STANDARD_REPORTS, &[];
    Pi => "pi", "pi-agent", STANDARD_REPORTS, &[];
    Goose => "goose", "Goose", STANDARD_REPORTS, &[];
    OpenClaw => "openclaw", "OpenClaw", STANDARD_REPORTS, &[];
    Kilo => "kilo", "Kilo", STANDARD_REPORTS, &[];
    Copilot => "copilot", "GitHub Copilot CLI", STANDARD_REPORTS, &[];
    Gemini => "gemini", "Gemini CLI", STANDARD_REPORTS, &[];
    Antigravity => "antigravity", "Antigravity", STANDARD_REPORTS, &[];
    Kimi => "kimi", "Kimi", STANDARD_REPORTS, &[];
    Qwen => "qwen", "Qwen", STANDARD_REPORTS, &[];
    Grok => "grok", "Grok", STANDARD_REPORTS, &[];
    ZCode => "zcode", "ZCode", STANDARD_REPORTS, &[];
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
