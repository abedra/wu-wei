use std::fs;
use std::path::PathBuf;

use crate::ui::{icon, theme};

/// Must match the `app_id` set on the window (see `main.rs`) and the
/// `.desktop` file's own name — GNOME (and other Wayland shells) resolves a
/// running window's task-switcher icon by matching its app ID against an
/// installed `.desktop` file's filename, not through any pixel data the
/// window itself provides. Native Wayland has no protocol for the latter.
pub const APP_ID: &str = "wu-wei";

/// Registers the app with the host desktop so task switchers, launchers and
/// docks show a real name and icon instead of a generic placeholder. `exe`
/// should be a stable path to the binary (a release build, not a `cargo run`
/// debug artifact that `cargo clean` will delete). Dispatches to the native
/// mechanism for the platform: XDG `.desktop` entries on Linux, an `.app`
/// bundle on macOS.
pub fn run(exe: &str) {
    #[cfg(target_os = "macos")]
    run_macos(exe);
    #[cfg(not(target_os = "macos"))]
    run_linux(exe);
}

/// Builds an `.app` bundle under `~/Applications` so Launchpad, Finder, the
/// Dock and the ⌘-Tab switcher resolve the app's name and icon. macOS keys all
/// of that off the enclosing bundle (its `Info.plist` and `.icns`), not off the
/// icon the window sets at runtime, so a real bundle on disk is what actually
/// replaces the generic terminal icon. The bundle's executable is a tiny shim
/// that `exec`s the release binary in place, so rebuilds are picked up without
/// reinstalling (mirroring how the Linux `.desktop` `Exec=` points at `exe`).
#[cfg(target_os = "macos")]
fn run_macos(exe: &str) {
    use std::os::unix::fs::PermissionsExt;

    let home = std::env::var_os("HOME").expect("HOME must be set to install an app bundle");
    let bundle = PathBuf::from(home).join("Applications").join("Wu Wei.app");
    let macos_dir = bundle.join("Contents/MacOS");
    let resources_dir = bundle.join("Contents/Resources");
    fs::create_dir_all(&macos_dir).expect("failed to create bundle MacOS directory");
    fs::create_dir_all(&resources_dir).expect("failed to create bundle Resources directory");

    let icon_path = resources_dir.join(format!("{APP_ID}.icns"));
    fs::write(&icon_path, icon::enso_icns(theme::ACCENT)).expect("failed to write .icns file");

    let plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>CFBundleName</key><string>Wu Wei</string>\n\
         \t<key>CFBundleDisplayName</key><string>Wu Wei</string>\n\
         \t<key>CFBundleIdentifier</key><string>com.aaronbedra.{APP_ID}</string>\n\
         \t<key>CFBundleExecutable</key><string>{APP_ID}</string>\n\
         \t<key>CFBundleIconFile</key><string>{APP_ID}</string>\n\
         \t<key>CFBundlePackageType</key><string>APPL</string>\n\
         \t<key>CFBundleShortVersionString</key><string>{version}</string>\n\
         \t<key>CFBundleVersion</key><string>{version}</string>\n\
         \t<key>NSHighResolutionCapable</key><true/>\n\
         \t<key>LSApplicationCategoryType</key>\
         <string>public.app-category.productivity</string>\n\
         </dict>\n\
         </plist>\n",
        version = env!("CARGO_PKG_VERSION"),
    );
    fs::write(bundle.join("Contents/Info.plist"), plist).expect("failed to write Info.plist");

    // A bundle launched from Finder/Dock starts with its working directory at
    // `/`, so the default relative `wu_wei.db` would resolve to an unwritable
    // `/wu_wei.db` and the app would panic before showing a window. Pin the
    // database to an absolute path at install time — resolved the same way the
    // app does, made absolute against the install-time directory — so the
    // installed app opens the very database you were running from. A path later
    // chosen in Settings still wins at runtime (it takes priority over this).
    let db_path = crate::db_bootstrap::resolve_db_path();
    let db_abs = {
        let p = PathBuf::from(&db_path);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir()
                .expect("failed to resolve current directory")
                .join(p)
        }
    };

    // Shim rather than a copy: `exec` the real binary so a rebuilt release
    // binary is reflected without reinstalling. `$@` forwards any arguments
    // LaunchServices passes through.
    let shim_path = macos_dir.join(APP_ID);
    fs::write(
        &shim_path,
        format!(
            "#!/bin/sh\nexport WU_WEI_DB_PATH=\"{}\"\nexec \"{exe}\" \"$@\"\n",
            db_abs.display(),
        ),
    )
    .expect("failed to write bundle launcher");
    fs::set_permissions(&shim_path, fs::Permissions::from_mode(0o755))
        .expect("failed to mark bundle launcher executable");

    // Best-effort: nudge LaunchServices to register the bundle so it appears
    // in Launchpad/Spotlight and adopts the icon without a logout. Harmless if
    // the path ever moves.
    let _ = std::process::Command::new(
        "/System/Library/Frameworks/CoreServices.framework/Frameworks/\
         LaunchServices.framework/Support/lsregister",
    )
    .arg("-f")
    .arg(&bundle)
    .status();

    println!("installed {}", bundle.display());
}

/// Writes a `.desktop` entry and an icon file into the user's XDG data
/// directories so the desktop shell can find both. `exe` should be a stable
/// path to the binary (a release build, not a `cargo run` debug artifact
/// that `cargo clean` will delete).
#[cfg(not(target_os = "macos"))]
fn run_linux(exe: &str) {
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
