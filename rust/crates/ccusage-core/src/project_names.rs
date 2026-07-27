use std::collections::HashMap;

pub(crate) fn parse_project_aliases(raw: Option<&str>) -> HashMap<String, String> {
    raw.unwrap_or_default()
        .split(',')
        .filter_map(|pair| {
            let (key, value) = pair.split_once('=')?;
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                None
            } else {
                Some((key.to_string(), value.to_string()))
            }
        })
        .collect()
}

pub fn format_project_name(project: &str, aliases: &HashMap<String, String>) -> String {
    if let Some(alias) = aliases.get(project) {
        return alias.clone();
    }
    let parsed = parse_project_name(project);
    aliases.get(&parsed).cloned().unwrap_or(parsed)
}

fn parse_project_name(project: &str) -> String {
    if project.is_empty() || project == "unknown" {
        return "Unknown Project".to_string();
    }
    let mut cleaned = project.to_string();
    if is_windows_users_path(&cleaned) {
        let segments = cleaned.split('\\').collect::<Vec<_>>();
        if let Some(index) = segments.iter().position(|segment| *segment == "Users") {
            if index + 3 < segments.len() {
                cleaned = segments[index + 3..].join("-");
            } else if index + 2 < segments.len() {
                cleaned = segments[index + 2..].join("-");
            }
        }
    } else if cleaned.starts_with("-Users-") || cleaned.starts_with("/Users/") {
        let separator = if cleaned.starts_with("-Users-") {
            '-'
        } else {
            '/'
        };
        let segments = cleaned
            .split(separator)
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>();
        if let Some(index) = segments.iter().position(|segment| *segment == "Users") {
            if index + 3 < segments.len() {
                cleaned = segments[index + 3..].join("-");
            } else if index + 2 < segments.len() {
                cleaned = segments[index + 2..].join("-");
            }
        }
    } else {
        cleaned = cleaned
            .trim_matches(|ch| ch == '/' || ch == '\\' || ch == '-')
            .to_string();
    }
    if cleaned.split('-').count() >= 5
        && cleaned
            .chars()
            .all(|ch| ch.is_ascii_hexdigit() || ch == '-' || ch == '.')
    {
        let parts = cleaned.split('-').collect::<Vec<_>>();
        cleaned = parts[parts.len().saturating_sub(2)..].join("-");
    }
    if let Some((main, _)) = cleaned.split_once("--") {
        cleaned = main.to_string();
    }
    if cleaned.contains('-') && cleaned.len() > 20 {
        let meaningful = cleaned
            .split('-')
            .filter(|segment| {
                segment.len() > 2
                    && !matches!(
                        segment.to_ascii_lowercase().as_str(),
                        "dev"
                            | "development"
                            | "feat"
                            | "feature"
                            | "fix"
                            | "bug"
                            | "test"
                            | "staging"
                            | "prod"
                            | "production"
                            | "main"
                            | "master"
                            | "branch"
                    )
            })
            .collect::<Vec<_>>();
        if meaningful.len() >= 2 {
            let last_two = meaningful[meaningful.len() - 2..].join("-");
            cleaned = if last_two.len() >= 6 {
                last_two
            } else if meaningful.len() >= 3 {
                meaningful[meaningful.len() - 3..].join("-")
            } else {
                cleaned
            };
        }
    }
    let cleaned = cleaned.trim_matches(|ch| ch == '/' || ch == '\\' || ch == '-');
    if cleaned.is_empty() {
        project.to_string()
    } else {
        cleaned.to_string()
    }
}

fn is_windows_users_path(project: &str) -> bool {
    let bytes = project.as_bytes();
    (bytes.len() >= 10
        && bytes[1] == b':'
        && bytes[2] == b'\\'
        && bytes[3..].starts_with(b"Users\\"))
        || project.starts_with("\\Users\\")
}

pub fn short_model_name(model: &str) -> String {
    let model = model
        .strip_prefix("anthropic/claude-")
        .or_else(|| model.strip_prefix("claude-"))
        .unwrap_or(model);
    let parts = model.split('-').collect::<Vec<_>>();
    if parts.len() >= 3 && parts.last().is_some_and(|part| part.len() == 8) {
        return parts[..parts.len() - 1].join("-");
    }
    model.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_users_project_paths_without_dropping_project_name() {
        let aliases = HashMap::new();

        assert_eq!(
            format_project_name("/Users/alice/ccusage", &aliases),
            "ccusage"
        );
        assert_eq!(
            format_project_name("/Users/alice/Development/ccusage", &aliases),
            "ccusage"
        );
        assert_eq!(
            format_project_name("-Users-alice-ccusage", &aliases),
            "ccusage"
        );
    }
}
