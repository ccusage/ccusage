use std::{env, path::PathBuf};

pub fn home_dir() -> Option<PathBuf> {
    home_dir_from_env(
        env::var_os("HOME"),
        env::var_os("USERPROFILE"),
        env::var_os("HOMEDRIVE"),
        env::var_os("HOMEPATH"),
    )
}

fn home_dir_from_env(
    home: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
    home_drive: Option<std::ffi::OsString>,
    home_path: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(path) = non_empty_path(home) {
        return Some(path);
    }
    if let Some(path) = non_empty_path(user_profile) {
        return Some(path);
    }
    let mut drive = home_drive?;
    let path = home_path?;
    if drive.is_empty() || path.is_empty() {
        return None;
    }
    drive.push(path);
    Some(PathBuf::from(drive))
}

fn non_empty_path(path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = path?;
    (!path.is_empty()).then(|| PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn prefers_home_when_available() {
        let path = home_dir_from_env(
            Some(OsString::from("/home/user")),
            Some(OsString::from("C:\\Users\\runner")),
            None,
            None,
        );
        assert_eq!(path, Some(PathBuf::from("/home/user")));
    }

    #[test]
    fn falls_back_to_windows_user_profile_without_home() {
        let path = home_dir_from_env(None, Some(OsString::from("C:\\Users\\runner")), None, None);
        assert_eq!(path, Some(PathBuf::from("C:\\Users\\runner")));
    }

    #[test]
    fn falls_back_to_windows_home_drive_and_path() {
        let path = home_dir_from_env(
            None,
            None,
            Some(OsString::from("C:")),
            Some(OsString::from("\\Users\\runner")),
        );
        assert_eq!(path, Some(PathBuf::from("C:\\Users\\runner")));
    }
}
