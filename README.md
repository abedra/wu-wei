# Wu Wei

A fast, keyboard-driven GTD-style task manager, built as a native desktop app in Rust with [egui](https://github.com/emilk/egui).

Wu Wei stores tasks and projects in a local SQLite database, offers AI-assisted quick capture and a conversational task assistant (OpenAI or Anthropic), and can sync across machines over a shared folder (Dropbox, iCloud Drive, a network share, a USB stick — anything that behaves like a folder).

## Features

- **GTD-style perspectives** — Inbox, Today, Overdue, Completed, and per-project views.
- **Keyboard-first workflow** — nearly everything (navigation, capture, moving tasks, due dates, completing, deleting) is reachable without the mouse. See [Keyboard shortcuts](#keyboard-shortcuts).
- **Quick capture** — jot a task in plain English; with AI configured, it's parsed into a title, due date, project, and recurrence automatically.
- **Due dates in plain English** — the Set Due Date picker (`D`) keeps its quick options (Today, Tomorrow, This Weekend, …) and adds a field where, with AI configured, you can type a phrase like "next friday" or "in 3 weeks" and have it resolved to a date.
- **AI chat assistant** — a bottom panel where you can ask things like "roll all of my overdue tasks to today" and have it act directly on your tasks and projects.
- **Recurring tasks** — completing a repeating task spawns its next occurrence automatically. A repeat can be restricted to certain weekdays (e.g. "every weekday"): an occurrence that would land on an excluded day rolls forward to the next allowed one.
- **Multi-device sync** — an optional, folder-based sync with last-write-wins merging and tombstone-based deletes; no server required.
- **Google Calendar in Today** — optionally connect a Google Calendar to show today's events alongside today's tasks. See [Google Calendar](#google-calendar).
- **Local-first** — a single SQLite file; everything works fully offline. AI features are entirely optional.

## Installation

Each tagged release publishes an installer per OS on the
[Releases](https://github.com/abedra/wei-wu/releases) page:

| OS | Download | Install |
| --- | --- | --- |
| Linux (Debian/Ubuntu) | `wu-wei_<v>_amd64.deb` | `sudo dpkg -i wu-wei_*.deb` (or open with your software centre) |
| Linux (Fedora/RHEL) | `wu-wei-<v>.x86_64.rpm` | `sudo rpm -i wu-wei-*.rpm` |
| Linux (Arch) | `PKGBUILD` | `makepkg -si` in a directory containing it |
| Windows | `Wu-Wei-Setup-<v>.exe` | run it (per-machine install; click through the SmartScreen prompt — the build is not code-signed yet) |
| macOS | `Wu-Wei-<v>.dmg` | open it, drag **Wu Wei** to Applications, then **first launch only**: right-click the app → Open → Open (the build is not notarized yet) |

The database is created on first launch in a per-OS data directory (see below).
How the installers are built lives in [`packaging/`](packaging/README.md).

## Building from source

### Requirements

- [Rust](https://rustup.rs) (edition 2024, so `rustc >= 1.85`)
- A C compiler (`cc`) — needed to build bundled SQLite and rustls' `ring` backend

Run `make doctor` to check your toolchain.

```sh
git clone <repo-url> wu-wei
cd wu-wei
make run
```

This builds and launches the app with `cargo run`. On first launch it creates an application data directory and puts its SQLite database (`wu_wei.db`) there:

| OS | Location |
| --- | --- |
| Linux | `$XDG_DATA_HOME/wu-wei/` (default `~/.local/share/wu-wei/`) |
| macOS | `~/Library/Application Support/wu-wei/` |
| Windows | `%APPDATA%\wu-wei\` (default `C:\Users\<you>\AppData\Roaming\wu-wei\`) |

If a `wu_wei.db` from an older version is found in the current directory, it's moved into this directory automatically. Set `WU_WEI_DB_PATH` or the in-app Settings to use a different location.

### Configuration

Configuration is via environment variables, optionally loaded from a `.env` file in the project root (see `.env` — never committed). Supported variables:

| Variable | Purpose |
| --- | --- |
| `WU_WEI_DB_PATH` | Path to the SQLite database file (default: `wu_wei.db` in the per-OS application data directory — see [Getting started](#getting-started)) |
| `WU_WEI_LLM_PROVIDER` | `openai` (default) or `anthropic` |
| `WU_WEI_LLM_API_KEY` | API key for AI features; overrides `OPENAI_API_KEY` / `ANTHROPIC_API_KEY` |
| `WU_WEI_LLM_BASE_URL` | Overrides the provider's default base URL (e.g. to point at a local OpenAI-compatible server) |
| `WU_WEI_LLM_MODEL` | Overrides the provider's default model (`gpt-4o-mini` for OpenAI, `claude-opus-5` for Anthropic) |
| `WU_WEI_LLM_MAX_TOKENS` | Optional cap on a single response's length; leave unset to use the provider's own limit |

AI-assisted capture and chat are entirely optional — leave the LLM variables unset and those features are simply disabled. Everything can also be configured (and changed live) from the in-app Settings screen (`Cmd+,`), which takes priority over these environment variables. A sync folder is likewise configured there.

## Development

Common tasks are wrapped in the `Makefile`:

```sh
make doctor        # verify cargo/rustc/cc are on PATH
make run            # cargo run
make build          # debug build
make release        # release build
make test           # run the test suite
make fmt            # apply rustfmt
make fmt-check      # check formatting without writing changes
make clippy         # run clippy lints (all targets)
make check          # fmt-check + clippy + test
make clean          # cargo clean
make install-desktop # install a .desktop entry + icon (Linux app switchers)
make emit-icons     # write wu-wei.{png,icns,ico} to dist/
make package-linux  # build .deb + .rpm into dist/
```

Release installers for all three platforms are built by
[`.github/workflows/release.yml`](.github/workflows/release.yml); see
[`packaging/`](packaging/README.md).

## Project layout

```
src/
  main.rs            entry point, window setup
  app.rs              top-level egui layout (sidebar, detail panel, chat, task list)
  state.rs             AppState: application state and all mutations
  db_bootstrap.rs      resolves which database file to open at launch
  desktop_install.rs   `install-desktop` / `emit-icons` subcommands (desktop entry, bundle, icon files)
  domain/              core types: Task, Project
  db/                  SQLite schema and repositories
  llm/                 OpenAI/Anthropic providers, prompt construction, chat actions
  sync/                folder-based multi-device sync (sync.rs)
  calendar/            Google Calendar OAuth + read-only events fetch
  ui/                  egui views: sidebar, task list, detail panel, pickers, settings, chat
```

## Keyboard shortcuts

| Shortcut | Action |
| --- | --- |
| `Cmd+N` | Open quick capture |
| `Enter` (in quick capture) | Submit, parsed by AI if configured |
| `Shift+Enter` (in quick capture) | Submit literally, skipping AI parsing |
| `Cmd+Shift+N` | Open new project popup |
| `Cmd+,` | Open Settings |
| `Cmd+Shift+S` | Sync now |
| `Cmd+1` / `Cmd+2` / `Cmd+3` / `Cmd+4` | Switch to Inbox / Today / Completed / Overdue |
| `←` | Move keyboard focus to the sidebar |
| `↑` / `↓` | Move selection (sidebar or task list, depending on focus) |
| `Enter` | Toggle the detail panel for the highlighted task |
| `Space` | Toggle the highlighted task complete/incomplete |
| `M` | Open the "move to project" picker |
| `D` | Open the due-date picker |
| `E` | Open the estimate picker |
| `Cmd+Backspace` | Delete the highlighted task |
| `Escape` | Close whatever popup/picker is open |

## Sync

Sync is folder-based, not a live connection: each device keeps its own local database, and a sync run writes the device's full current state to `<folder>/<device-id>.json`, reads every other device's file, and merges the result locally. Conflicts resolve by latest `updated_at`, except deletions always win regardless of timestamp. Set a shared folder path in Settings to enable it; the app auto-syncs periodically and on launch.

## Google Calendar

The Today view can show today's events from a Google Calendar (read-only, primary calendar only). Since this talks to Google's API on your behalf, it needs an OAuth client from your own Google Cloud project:

1. In the [Google Cloud Console](https://console.cloud.google.com/), create a project (or use an existing one) and enable the **Google Calendar API**.
2. Configure the OAuth consent screen (External is fine for personal use; you don't need to submit it for verification to use it yourself).
3. Create an OAuth client ID of type **Desktop app**. Desktop app clients accept a loopback redirect (`http://127.0.0.1:<any port>/`) without registering an exact port, which is what Wu Wei uses to receive the sign-in response.
4. In Wu Wei's Settings (`Cmd+,`) → Calendar, paste the client ID and client secret, then click **Connect Google Calendar**. Your browser opens Google's consent screen; approving it hands control back to the app automatically.

Once connected, Today's events appear above the task list whenever you're on the Today perspective, refreshing automatically on an interval you set in Settings → Calendar → **Refresh every** (default 5 minutes, 1–1440). The refresh only runs while the Today view is open. Disconnecting from Settings revokes nothing on Google's side — it just clears Wu Wei's saved tokens.

