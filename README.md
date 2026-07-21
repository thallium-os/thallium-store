# Thallium Store

> The app store for Thallium 81 — one place to find and install software from apt, Flathub, GitHub releases and AppImages, ranked by trust and installed through UNI.

## What it does

Thallium Store merges four package sources into a single catalog. Search once and every source answers; each app shows every channel it ships on, with the safest option recommended. Installs, removals and updates run through [UNI](https://github.com/dronzer-tb/UNI), the system's package authority — the store never shells out to `apt` or `flatpak` on its own.

- **Unified catalog** — apt, Flathub, GitHub releases and AppImage results merged and de-duplicated, each variant tagged with its trust level (sandboxed, system access, verified, unverified).
- **Editorial home** — curated collections and a featured carousel that paints real Flathub artwork behind each pick.
- **Rich detail pages** — screenshots, description, every install source with its exact UNI command, source-language breakdown for open-source apps (via GitHub linguist), and metadata.
- **Apps view** — everything installed on the system, plus live install/remove/update activity.
- **Fast and local** — app icons and enriched details are cached on disk, so the store paints instantly after first load and touches the network only for search, artwork and metadata.

## Quick Start

```bash
git clone https://github.com/dronzer-tb/thallium-store.git
cd thallium-store
./scripts/dev-run
```

`dev-run` builds the workspace and launches the Quickshell UI against a freshly built backend, using real UNI. First launch shows a short setup walkthrough.

## Installation

Build a Debian package and install it:

```bash
./scripts/build-deb
sudo apt install ./thallium-store_*.deb
```

Or install into your user prefix without packaging:

```bash
./scripts/install-local
```

## Usage

The store is a Quickshell shell; launch it with the bundled wrapper (added to `PATH` by the package):

```bash
thallium-store
```

- **Home** — curated picks and collections.
- **Search** — click the search icon in the ribbon; every source is queried as you type, press Enter for full results.
- **Apps** — installed software and live operation activity (retry/cancel from here).
- **Updates** — available updates from UNI.
- **Settings** — cache storage controls, replay the first-run walkthrough, store info.

## Configuration

Set via environment variables (the wrappers in `scripts/` set sensible defaults):

| Variable | Default | Purpose |
| --- | --- | --- |
| `THALLIUM_STORE_FAKE_UNI` | `0` | `1` runs a safe simulator — progress and state without real package mutations. |
| `THALLIUM_STORE_BACKEND` | `thallium-store-backend` on `PATH` | Path to the backend binary. |
| `THALLIUM_STORE_UNI` | bundled `vendor/uni/uni` | Path to the UNI binary. |
| `UNI_PRIVILEGE_BACKEND` | `pkexec` | Privilege escalation for apt/system flatpak mutations (`pkexec` or `sudo`). |

State lives under `~/.local/share/thallium-store/` (SQLite DB, icon cache) and `~/.config/thallium-store/settings.json`.

## Architecture

A Rust backend and a QML frontend talk newline-delimited JSON-RPC over a per-user Unix socket. The backend is a Cargo workspace:

| Crate | Responsibility |
| --- | --- |
| `store-backend` | JSON-RPC daemon, socket server, request routing, caches, settings. |
| `store-catalog` | Source providers (apt, Flathub, GitHub, AppImage), merge/rank, detail enrichment. |
| `store-uni` | UNI adapter — install/remove/update backends, privilege escalation, progress streaming. |
| `store-db` | SQLite operation history. |
| `store-core` | Shared models and the trust-ranking logic. |

The UI is a single Quickshell/QML shell (`ui/shell.qml`) in the Thallium 81 design language (Everforest palette, chamfered HUD surfaces). Curated catalog data lives in `data/`.

## Contributing

```bash
cargo build          # build the workspace
cargo test           # run tests
./scripts/check      # fmt + clippy + tests
```

UNI is vendored under `vendor/uni/`. Heavy builds and CI run in GitHub Actions.

## License

[GNU General Public License v3.0](LICENSE) or later.
