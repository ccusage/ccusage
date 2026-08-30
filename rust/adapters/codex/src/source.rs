pub(crate) const CODEX_UNCATEGORIZED_SOURCE: &str = "Uncategorized";

pub(crate) fn normalize_codex_originator(originator: Option<&str>) -> String {
    let Some(originator) = originator else {
        return CODEX_UNCATEGORIZED_SOURCE.to_string();
    };
    if originator.trim().is_empty() {
        return CODEX_UNCATEGORIZED_SOURCE.to_string();
    }
    match originator {
        "codex-tui" | "codex_cli_rs" => "CLI".to_string(),
        "codex_exec" => "Exec".to_string(),
        "Codex Desktop" | "codex_work_desktop" => "Desktop App".to_string(),
        "codex_vscode" => "VS Code".to_string(),
        "codex_python_sdk" => "SDK".to_string(),
        _ => originator.to_string(),
    }
}
