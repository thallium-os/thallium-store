# Thallium Store

> A desktop-independent store for Debian-based systems — one place to find and install software from apt, Flathub, GitHub releases and AppImages, ranked by trust and installed through UNI.

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

`dev-run` builds the workspace, starts the local web frontend, and opens it in your default browser against a freshly built backend using real UNI. Set `THALLIUM_STORE_NO_OPEN=1` to print the local URL without opening a browser.

## Installation

Build a Debian package and install it:

```bash
./scripts/build-deb
sudo apt install ./dist/thallium-store_*.deb
```

Or install into your user prefix without packaging:

```bash
./scripts/install-local
```

## Usage

Launch the store with the bundled wrapper (added to `PATH` by the package):

```bash
thallium-store
```

The wrapper starts an HTTP server bound to a random **loopback-only** port and opens the store in the default browser. The server is not exposed to the network, accepts mutations only from its per-launch session token, and exits after 15 minutes without browser activity. It works on GNOME, KDE Plasma, Xfce, Cinnamon, Sway, Hyprland, and other X11 or Wayland sessions; Quickshell is not installed or required.

- **Home** — curated picks and collections.
- **Search** — use the header search box; every configured source is queried together.
- **Apps** — software currently installed through UNI or Flatpak, with removal controls.
- **Updates** — available updates from UNI.
- **Activity** — live and previous package operations, including cancellation controls.
- **Settings** — backend health, compatibility information, and icon-cache controls.

## Configuration

Set via environment variables (the wrappers in `scripts/` set sensible defaults):

| Variable | Default | Purpose |
| --- | --- | --- |
| `THALLIUM_STORE_FAKE_UNI` | `0` | `1` runs a safe simulator — progress and state without real package mutations. |
| `THALLIUM_STORE_UNI` | bundled `vendor/uni/uni` | Path to the UNI binary. |
| `UNI_PRIVILEGE_BACKEND` | `pkexec` | Privilege escalation for apt/system flatpak mutations (`pkexec` or `sudo`). |
| `THALLIUM_STORE_NO_OPEN` | unset | Set to any value to print the local URL without opening a browser. |
| `THALLIUM_STORE_WEB_IDLE_SECONDS` | `900` | Shut down the local server after this many seconds without requests (minimum 30). |

State lives under `~/.local/share/thallium-store/` (SQLite DB, icon cache) and `~/.config/thallium-store/settings.json`.

## Architecture

A Rust backend embeds and serves a dependency-free HTML/CSS/JavaScript frontend over loopback HTTP. Browser requests call the same JSON-RPC handlers used by the Unix-socket CLI. The backend is a Cargo workspace:

| Crate | Responsibility |
| --- | --- |
| `store-backend` | JSON-RPC daemon, socket server, request routing, caches, settings. |
| `store-catalog` | Source providers (apt, Flathub, GitHub, AppImage), merge/rank, detail enrichment. |
| `store-uni` | UNI adapter — install/remove/update backends, privilege escalation, progress streaming. |
| `store-db` | SQLite operation history. |
| `store-core` | Shared models and the trust-ranking logic. |

The active frontend lives in `web/` and is embedded into the backend binary at build time, so the Debian package has no web-server or GUI-toolkit runtime dependency. The previous Quickshell client remains under `ui/` as a design and migration reference. Curated catalog data lives in `data/`.

## Contributing

```bash
cargo build          # build the workspace
cargo test           # run tests
./scripts/check      # fmt + clippy + tests
```

UNI is vendored under `vendor/uni/`. Heavy builds and CI run in GitHub Actions.

## License

[GNU General Public License v3.0](LICENSE) or later.
