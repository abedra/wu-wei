use std::path::PathBuf;

const FILE_NAME: &str = "db_path";

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home = std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .expect("HOME (or USERPROFILE on Windows) must be set");
            PathBuf::from(home).join(".config")
        })
        .join("wu-wei")
}

/// Which database file to open at startup: a path remembered from an
/// earlier "change database" in Settings (see `remember_db_path`) takes
/// priority, then `WU_WEI_DB_PATH`, then the `wu_wei.db` default. This can't
/// live inside the database itself — it's what decides which database to
/// open in the first place.
pub fn resolve_db_path() -> String {
    if let Ok(remembered) = std::fs::read_to_string(config_dir().join(FILE_NAME)) {
        let remembered = remembered.trim();
        if !remembered.is_empty() {
            return remembered.to_string();
        }
    }
    std::env::var("WU_WEI_DB_PATH").unwrap_or_else(|_| "wu_wei.db".to_string())
}

/// Remembers `path` as the database to open on the next launch. A no-op
/// during `cargo test` so the test suite never touches the real user's
/// config directory.
pub fn remember_db_path(path: &str) {
    if cfg!(test) {
        return;
    }
    let dir = config_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let _ = std::fs::write(dir.join(FILE_NAME), path);
}
