struct OptionSpec {
    names: &'static [&'static str],
    takes_value: bool,
    shared: bool,
}

const OPTIONS: &[OptionSpec] = &[
    OptionSpec {
        names: &["-s", "--since"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["-u", "--until"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["--last"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["-m", "--mode"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["--debug-samples"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["-o", "--order"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["-z", "--timezone"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["-q", "--jq"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["--config"],
        takes_value: true,
        shared: true,
    },
    OptionSpec {
        names: &["-j", "--json"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["-d", "--debug"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["-b", "--breakdown"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["-O", "--offline"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["--no-offline"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["--color"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["--no-color"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["--compact"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["--single-thread"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["--no-cost"],
        takes_value: false,
        shared: true,
    },
    OptionSpec {
        names: &["-p", "--project"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--project-aliases"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["-w", "--start-of-week"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["-i", "--id"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["-t", "--token-limit"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["-n", "--session-length"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["-B", "--visual-burn-rate"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--cost-source"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--refresh-interval"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--context-low-threshold"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--context-medium-threshold"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--speed"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--pi-path"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--open-claw-path"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["--sections"],
        takes_value: true,
        shared: false,
    },
    OptionSpec {
        names: &["-a", "--active"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["-r", "--recent"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["--instances"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["--all"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["--by-agent"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["--include-claude-agent"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["--cache"],
        takes_value: false,
        shared: false,
    },
    OptionSpec {
        names: &["--no-cache"],
        takes_value: false,
        shared: false,
    },
];

fn option(arg: &str) -> Option<&'static OptionSpec> {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    OPTIONS.iter().find(|option| option.names.contains(&name))
}

pub fn option_takes_value(arg: &str) -> bool {
    option(arg).is_some_and(|option| option.takes_value)
}

pub fn is_shared_flag(arg: &str) -> bool {
    option(arg).is_some_and(|option| option.shared)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn value_options_include_source_paths_and_session_ids() {
        for name in ["-i", "--id", "--open-claw-path", "--open-claw-path=daily"] {
            assert!(option_takes_value(name), "{name}");
            assert!(!is_shared_flag(name), "{name}");
        }
    }

    #[test]
    fn shared_switches_and_value_options_are_distinct() {
        assert!(is_shared_flag("--offline=false"));
        assert!(!option_takes_value("--offline"));
        assert!(is_shared_flag("--since=20260101"));
        assert!(option_takes_value("--since"));
        assert!(!option_takes_value("--unknown"));
        assert!(!is_shared_flag("--unknown"));
    }
}
