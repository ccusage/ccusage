struct OptionSpec {
    arity: OptionArity,
    shared: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OptionArity {
    Switch,
    Value,
    CommandDependent,
}

fn option(arg: &str) -> Option<OptionSpec> {
    let name = arg.split_once('=').map_or(arg, |(name, _)| name);
    let (takes_value, shared) = match name {
        "-h" | "--help" | "-v" | "-V" | "--version" => (false, false),
        "-s" | "--since" => (true, true),
        "-u" | "--until" => (true, true),
        "--last" => (true, true),
        "-m" | "--mode" => (true, true),
        "--debug-samples" => (true, true),
        "-o" | "--order" => (true, true),
        "-z" | "--timezone" => (true, true),
        "-q" | "--jq" => (true, true),
        "--config" => (true, true),
        "-j" | "--json" => (false, true),
        "-d" | "--debug" => (false, true),
        "-b" | "--breakdown" => (false, true),
        "-O" | "--offline" => (false, true),
        "--no-offline" => (false, true),
        "--color" => (false, true),
        "--no-color" => (false, true),
        "--compact" => (false, true),
        "--single-thread" => (false, true),
        "--no-cost" => (false, true),
        "-p" | "--project" => (true, false),
        "--project-aliases" => (true, false),
        "-w" | "--start-of-week" => (true, false),
        "-i" | "--id" => (true, false),
        "-t" | "--token-limit" => (true, false),
        "-n" | "--session-length" => (true, false),
        "-B" | "--visual-burn-rate" => (true, false),
        "--cost-source" => (true, false),
        "--refresh-interval" => (true, false),
        "--context-low-threshold" => (true, false),
        "--context-medium-threshold" => (true, false),
        "--speed" => (true, false),
        "--pi-path" => (true, false),
        "--open-claw-path" => (true, false),
        "--sections" => (true, false),
        "-a" | "--active" => (false, false),
        "-r" | "--recent" => (false, false),
        "--instances" => (false, false),
        "--all" => (false, false),
        "--by-agent" => (false, false),
        "--include-claude-agent" => (false, false),
        "--cache" => (false, false),
        "--no-cache" => (false, false),
        _ => return None,
    };
    Some(OptionSpec {
        arity: if name == "-i" {
            OptionArity::CommandDependent
        } else if takes_value {
            OptionArity::Value
        } else {
            OptionArity::Switch
        },
        shared,
    })
}

pub fn option_arity(arg: &str) -> Option<OptionArity> {
    option(arg).map(|option| option.arity)
}

pub fn option_takes_value(arg: &str) -> bool {
    matches!(
        option_arity(arg),
        Some(OptionArity::Value | OptionArity::CommandDependent)
    )
}

pub fn is_shared_flag(arg: &str) -> bool {
    option(arg).is_some_and(|option| option.shared)
}

pub fn command_tokens(args: &[String]) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        if arg.starts_with('-') {
            if option_takes_value(arg) && !arg.contains('=') {
                index += 2;
            } else {
                index += 1;
            }
            continue;
        }
        tokens.push(arg.clone());
        index += 1;
    }
    tokens
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
