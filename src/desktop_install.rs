use std::fs;
use std::path::PathBuf;

use crate::ui::{icon, theme};

/// Must match the `app_id` set on the window (see `main.rs`) and the
/// `.desktop` file's own name — GNOME (and other Wayland shells) resolves a
/// running window's task-switcher icon by matching its app ID against an
/// installed `.desktop` file's filename, not through any pixel data the
/// window itself provides. Native Wayland has no protocol for the latter.
pub const APP_ID: &str = "wu-wei";

/// Writes a `.desktop` entry and an icon file into the user's XDG data
/// directories so the desktop shell can find both. `exe` should be a stable
/// path to the binary (a release build, not a `cargo run` debug artifact
/// that `cargo clean` will delete).
pub fn run(exe: &str) {
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let home =
                std::env::var_os("HOME").expect("HOME must be set to install a desktop entry");
            PathBuf::from(home).join(".local/share")
        });

    let hicolor_dir = data_home.join("icons/hicolor");
    let icon_dir = hicolor_dir.join("256x256/apps");
    fs::create_dir_all(&icon_dir).expect("failed to create icon directory");
    let icon_path = icon_dir.join(format!("{APP_ID}.png"));
    fs::write(&icon_path, icon::enso_png(256, theme::ACCENT)).expect("failed to write icon file");

    // Without an `index.theme` marker here, GTK won't treat this directory
    // as part of the "hicolor" theme at all — icon lookups silently fall
    // through to a placeholder rather than scanning it, even though the
    // system-wide hicolor theme (which this one is meant to extend) already
    // declares the `256x256/apps` directory itself. Confirmed by testing
    // `Gtk.IconTheme.has_icon` directly: it's `False` without this file
    // present, `True` with it. Only written if absent, so a hand-edited one
    // (or a fuller one dropped in by something else) is left alone.
    let index_theme_path = hicolor_dir.join("index.theme");
    if !index_theme_path.exists() {
        fs::write(
            &index_theme_path,
            "[Icon Theme]\n\
             Name=Hicolor\n\
             Comment=Fallback icon theme\n\
             Hidden=true\n\
             Directories=256x256/apps\n\
             \n\
             [256x256/apps]\n\
             Size=256\n\
             Context=Applications\n\
             Type=Fixed\n",
        )
        .expect("failed to write hicolor index.theme");
    }

    let apps_dir = data_home.join("applications");
    fs::create_dir_all(&apps_dir).expect("failed to create applications directory");
    let desktop_path = apps_dir.join(format!("{APP_ID}.desktop"));
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Wu Wei\n\
         Comment=A GTD-style task manager\n\
         Exec={exe}\n\
         Icon={APP_ID}\n\
         Terminal=false\n\
         Categories=Office;\n\
         StartupWMClass={APP_ID}\n"
    );
    fs::write(&desktop_path, entry).expect("failed to write .desktop file");

    // Best-effort cache refresh: some shells pick up new entries without
    // this, others need it nudged. Harmless if the tools aren't installed.
    let _ = std::process::Command::new("update-desktop-database")
        .arg(&apps_dir)
        .status();
    let _ = std::process::Command::new("gtk-update-icon-cache")
        .arg(&hicolor_dir)
        .status();

    println!(
        "installed {} and {}",
        desktop_path.display(),
        icon_path.display()
    );
}
