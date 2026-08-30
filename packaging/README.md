# Packaging

Installers are built per-OS on native GitHub Actions runners by
`.github/workflows/release.yml`, triggered by pushing a `vX.Y.Z` tag (the
workflow checks the tag matches `Cargo.toml`'s `version`). `workflow_dispatch`
runs the same build for any version string without publishing a release.

All three installers get their icon from the binary itself:
`wu-wei emit-icons <dir>` writes `wu-wei.png` / `wu-wei.icns` / `wu-wei.ico`
(the mark is drawn procedurally in `src/ui/icon.rs`).

| OS | Output | Tooling | Notes |
| --- | --- | --- | --- |
| Linux | `.deb`, `.rpm`, `PKGBUILD` | `cargo-deb`, `cargo-generate-rpm` | metadata lives in `Cargo.toml` (`[package.metadata.deb]` / `[package.metadata.generate-rpm]`) |
| Windows | `Wu-Wei-Setup-<v>.exe` | NSIS (`packaging/windows/installer.nsi`) | per-machine install to `Program Files`, UAC prompt, Add/Remove Programs entry |
| macOS | `Wu-Wei-<v>.dmg` | `packaging/macos/make-bundle.sh` + `create-dmg` | universal (x86_64 + arm64), **unsigned** |

## Building locally

### Linux (works on any Linux box)

```sh
cargo install cargo-deb cargo-generate-rpm   # once
make package-linux                           # -> dist/*.deb, dist/*.rpm
```

Install/remove the `.deb`:

```sh
sudo dpkg -i dist/wu-wei_*.deb
sudo dpkg -r wu-wei
```

The package installs the binary to `/usr/bin/wu-wei`, a `.desktop` entry, and
a hicolor icon. User data still goes to `~/.local/share/wu-wei/` at runtime and
is left untouched on removal.

### Windows

Needs a Windows machine (or runner) with Rust and NSIS:

```pwsh
cargo build --release
.\target\release\wu-wei.exe emit-icons dist
makensis /DVERSION=0.1.0 /DSRCDIR=target\release /DICON=dist\wu-wei.ico `
  /DOUTFILE=dist\Wu-Wei-Setup-0.1.0.exe packaging\windows\installer.nsi
```

### macOS

Needs a Mac with Rust:

```sh
cargo build --release
packaging/macos/make-bundle.sh target/release/wu-wei 0.1.0 dist
# then, for a .dmg:
brew install create-dmg
create-dmg --volname "Wu Wei" --app-drop-link 420 180 \
  "dist/Wu-Wei-0.1.0.dmg" "dist/Wu Wei.app"
```

## Not wired up yet

- **macOS signing + notarization.** The bundle is ad-hoc signed, so first launch
  is right-click → Open past a Gatekeeper warning. Adding a Developer ID cert +
  notarization (`rcodesign` or `xcrun notarytool`) to the `macos` job is the fix.
- **Windows Authenticode signing.** Unsigned installers trip a dismissable
  SmartScreen prompt. Needs a code-signing cert (hardware token / cloud HSM).
- AppImage / Flatpak / Snap, a Homebrew tap, a winget manifest.
