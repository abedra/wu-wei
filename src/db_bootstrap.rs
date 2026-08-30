use std::path::{Path, PathBuf};

const FILE_NAME: &str = "db_path";
/// The per-user directory this app owns, under the platform's config and
/// data roots. Kebab-case to match the window app id (see `desktop_install`).
const APP_DIR: &str = "wu-wei";
const DB_FILE_NAME: &str = "wu_wei.db";

/// The user's home directory. `USERPROFILE` is the Windows spelling; `HOME`
/// is set there too under most shells, but not universally, so try both.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .expect("HOME (or USERPROFILE on Windows) must be set")
}

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".config"))
        .join(APP_DIR)
}

/// The platform's root for user-specific application *data* (as opposed to
/// cache or config): where a single-binary distribution should keep its
/// SQLite database.
///
///   - macOS:   `~/Library/Application Support`
///   - Windows: `%APPDATA%` (the roaming profile), or `~/AppData/Roaming`
///   - Linux:   `$XDG_DATA_HOME`, or `~/.local/share`
#[cfg(target_os = "macos")]
fn platform_data_root() -> PathBuf {
    home().join("Library").join("Application Support")
}

#[cfg(target_os = "windows")]
fn platform_data_root() -> PathBuf {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join("AppData").join("Roaming"))
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_data_root() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(".local").join("share"))
}

/// This app's own data directory: `wu-wei/` under the platform data root.
/// Not created here — `default_db_path` creates it on first run, and
/// `db::open` creates the parent of whatever path it's handed.
pub fn data_dir() -> PathBuf {
    platform_data_root().join(APP_DIR)
}

/// Where the database lives when nothing overrides it: `wu_wei.db` inside
/// `data_dir()`. Creates that directory on first launch. If it can't be
/// created (a read-only home, say), falls back to a bare relative
/// `wu_wei.db` so the app still starts against the working directory.
fn default_db_path() -> String {
    let dir = data_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return DB_FILE_NAME.to_string();
    }
    dir.join(DB_FILE_NAME).to_string_lossy().into_owned()
}

/// One-time move for databases created before the app used a per-OS data
/// directory: if a `wu_wei.db` sits in the current working directory and
/// nothing is at the new default location yet, relocate it there so the
/// existing tasks and projects aren't left behind. `rename` fails across
/// filesystems, so fall back to copy-then-delete.
fn adopt_legacy_db(legacy: &Path, target: &Path) {
    if target == legacy || target.exists() || !legacy.exists() {
        return;
    }
    if let Some(parent) = target.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if std::fs::rename(legacy, target).is_err() && std::fs::copy(legacy, target).is_ok() {
        let _ = std::fs::remove_file(legacy);
    }
}

/// Which database file to open at startup: a path remembered from an
/// earlier "change database" in Settings (see `remember_db_path`) takes
/// priority, then `WU_WEI_DB_PATH`, then `wu_wei.db` in the per-OS
/// application data directory (see `data_dir`). This can't live inside the
/// database itself — it's what decides which database to open in the first
/// place.
pub fn resolve_db_path() -> String {
    if let Ok(remembered) = std::fs::read_to_string(config_dir().join(FILE_NAME)) {
        let remembered = remembered.trim();
        if !remembered.is_empty() {
            return remembered.to_string();
        }
    }
    if let Ok(env) = std::env::var("WU_WEI_DB_PATH")
        && !env.trim().is_empty()
    {
        return env;
    }
    let default = default_db_path();
    adopt_legacy_db(Path::new(DB_FILE_NAME), Path::new(&default));
    default
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adopt_moves_a_legacy_db_when_the_target_is_absent() {
        let tmp = std::env::temp_dir().join(format!("wu-wei-adopt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let legacy = tmp.join("old.db");
        let target = tmp.join("data").join("wu_wei.db");
        std::fs::write(&legacy, b"payload").unwrap();

        adopt_legacy_db(&legacy, &target);

        assert!(!legacy.exists(), "legacy file should have moved");
        assert_eq!(std::fs::read(&target).unwrap(), b"payload");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn adopt_leaves_an_existing_target_untouched() {
        let tmp = std::env::temp_dir().join(format!("wu-wei-adopt-keep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let legacy = tmp.join("old.db");
        let target = tmp.join("wu_wei.db");
        std::fs::write(&legacy, b"old").unwrap();
        std::fs::write(&target, b"current").unwrap();

        adopt_legacy_db(&legacy, &target);

        assert_eq!(std::fs::read(&target).unwrap(), b"current");
        assert!(legacy.exists(), "legacy file is left where it is");
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn data_dir_sits_under_the_app_folder() {
        assert!(data_dir().ends_with(APP_DIR));
    }
}
