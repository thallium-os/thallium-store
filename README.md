# Thallium Store

> A native Qt store for Debian-based systems — one place to find and install software from apt, Flathub, GitHub releases and AppImages, ranked by trust and installed through UNI.

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
sudo apt install cmake g++ qt6-base-dev qt6-declarative-dev \
  qml6-module-qtqml qml6-module-qtqml-models \
  qml6-module-qtqml-workerscript \
  qml6-module-qtquick qml6-module-qtquick-controls \
  qml6-module-qtquick-layouts qml6-module-qtquick-shapes \
  qml6-module-qtquick-templates qml6-module-qtquick-window \
  qt6-qpa-plugins
./scripts/dev-run
```

`dev-run` builds the Rust workspace and launches the native Qt 6 client against a freshly built backend using real UNI. The interface is Qt Quick/QML; it does not embed Chromium, Node, Electron, or a webview.

## Installation

On Debian 12, Debian 13, Ubuntu 24.04, and compatible Ubuntu derivatives, add
the signed Thallium Store repository and install the native application with
one command:

```bash
curl -fsSL https://store-qt.thallium81.dev/install.sh | sh
```

The source remains installed, so new releases arrive through the system's
normal `apt update` and upgrade process. See
[`docs/apt-repository.md`](docs/apt-repository.md) for supported systems and
repository security details.

### Build locally

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

The wrapper launches the native Qt executable on GNOME, KDE Plasma, Xfce, Cinnamon, Sway, Hyprland, and other X11 or Wayland sessions. Quickshell is not installed or required. The loopback browser client remains available as an automatic recovery path if the native payload is missing.

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
| `THALLIUM_STORE_NATIVE` | installed native client | Override the native Qt executable path. |
| `THALLIUM_STORE_NO_OPEN` | unset | Set to any value to print the local URL without opening a browser. |
| `THALLIUM_STORE_WEB_IDLE_SECONDS` | `900` | Shut down the local server after this many seconds without requests (minimum 30). |

State lives under `~/.local/share/thallium-store/` (SQLite DB, icon cache) and `~/.config/thallium-store/settings.json`.

## Architecture

A native Qt 6/QML client and Rust backend communicate through newline-delimited JSON-RPC on a per-user Unix socket. The backend is a Cargo workspace:

| Crate | Responsibility |
| --- | --- |
| `store-backend` | JSON-RPC daemon, socket server, request routing, caches, settings. |
| `store-catalog` | Source providers (apt, Flathub, GitHub, AppImage), merge/rank, detail enrichment. |
| `store-uni` | UNI adapter — install/remove/update backends, privilege escalation, progress streaming. |
| `store-db` | SQLite operation history. |
| `store-core` | Shared models and the trust-ranking logic. |

The active frontend lives in `ui/shell.qml`. A small Qt/C++ host under `native/` owns the application window, starts the backend, exposes themed desktop icons, and bridges QML requests to the Unix socket. The embedded frontend under `web/` remains a fallback. Curated catalog data lives in `data/`.

## Contributing

```bash
cargo build          # build the workspace
cargo test           # run tests
./scripts/build-native # build the Rust backend and native Qt client
./scripts/check      # fmt + clippy + tests
```

UNI is vendored under `vendor/uni/`. Heavy builds and CI run in GitHub Actions.

## License

[GNU General Public License v3.0](LICENSE) or later.
