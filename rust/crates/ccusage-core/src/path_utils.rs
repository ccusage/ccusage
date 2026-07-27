use std::path::PathBuf;

pub fn expand_home_path(raw: &str) -> PathBuf {
    if raw == "~"
        && let Some(home) = crate::home::home_dir()
    {
        return home;
    }
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = crate::home::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(raw)
}
