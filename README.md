# Thallium Store

> The app store for Thallium 81 — one place to find and install software from apt, Flathub, GitHub releases and AppImages, ranked by trust. Three interchangeable frontends over one Rust backend: Quickshell, native Qt 6, or a browser.

## What it does

Thallium Store merges four package sources into a single catalog. Search once and every source answers; each app shows every channel it ships on, with the safest option recommended. Installs, removals and updates are driven by the backend itself — it runs `apt-get` and `flatpak` directly, escalating through polkit. UNI was vendored once and is no longer invoked; see [`dronzer-tb/uni`](https://github.com/dronzer-tb/uni) for the standalone tool.

- **Unified catalog** — apt, Flathub, GitHub releases and AppImage results merged and de-duplicated, each variant tagged with its trust level (sandboxed, system access, verified, unverified).
- **Editorial home** — curated collections and a featured carousel that paints real Flathub artwork behind each pick.
- **Rich detail pages** — screenshots, description, every install source with the exact command it will run, source-language breakdown for open-source apps (via GitHub linguist), and metadata.
- **Apps view** — everything installed on the system, plus live install/remove/update activity.
- **Fast and local** — app icons and enriched details are cached on disk, so the store paints instantly after first load and touches the network only for search, artwork and metadata.
- **Three frontends, one backend** — Quickshell for the Thallium desktop, a native Qt 6 client for everywhere else, and a loopback browser client that needs no toolkit at all. See [Frontends](#frontends).

## Frontends

The backend is the product; the UI is replaceable. All three frontends drive the
same RPC methods — the Qt hosts over the Unix socket, the web client over
loopback HTTP — and offer the same views, so which one you run is a deployment
question, not a feature question.

| Frontend | Runtime dependency | Entry point | Lives on |
| --- | --- | --- | --- |
| **Quickshell** | `quickshell` | `quickshell --path ui/shell.qml` | `main` |
| **Native Qt 6** | `qt6-base-dev`, `qt6-declarative-dev`, CMake | `thallium-store-native` | `native/qt6` |
| **Web** | any browser | `thallium-store-backend --web` | `compat/no-quickshell` (also on `native/qt6`) |

**Quickshell** is the default and the one Thallium 81 ships. It assumes the
Thallium desktop is present.

**Native Qt 6** exists because Quickshell is a Thallium dependency, not a Debian
one — a store that only runs under its own desktop cannot be installed on a plain
Debian box to *get* that desktop. `native/` is a small C++ host that embeds the
same `ui/shell.qml`, exposes desktop theme icons, and bridges QML to the
backend's Unix socket. No Quickshell, Electron, Chromium, Node or webview.

```bash
sudo apt install cmake g++ qt6-base-dev qt6-declarative-dev \
  qml6-module-qtqml qml6-module-qtqml-models qml6-module-qtqml-workerscript \
  qml6-module-qtquick qml6-module-qtquick-controls \
  qml6-module-qtquick-layouts qml6-module-qtquick-shapes \
  qml6-module-qtquick-templates qml6-module-qtquick-window qt6-qpa-plugins
./scripts/build-native          # -> target/native/thallium-store-native
```

**Web** is the floor: `--web` binds an ephemeral port on `127.0.0.1`, mints a
per-session UUID that every request must present as `x-thallium-token`, serves
`web/` (compiled into the binary via `include_str!`), and exits once idle. It
needs no graphical toolkit at all, which makes it the recovery path — the
`thallium-store` wrapper on `native/qt6` falls back to it when the native binary
is missing.

On those branches the `thallium-store` wrapper picks a frontend in order:
`$THALLIUM_STORE_NATIVE`, then `~/.local/libexec/`, `/usr/libexec/`,
`target/native/`, then `--web`.

> **Branch state.** `native/qt6` (+13) and `compat/no-quickshell` (+1) both fork
> at `1a249b1` and are **26 commits behind `main`**. Their build instructions
> predate `2b1ab43 build: un-vendor uni`, so they still carry `vendor/uni/` and
> the wrapper still resolves a `uni` binary. Rebase before shipping either.

## Quick Start

```bash
git clone https://github.com/dronzer-tb/thallium-store.git
cd thallium-store
./scripts/dev-run
```

`dev-run` builds the workspace and launches the Quickshell UI against a freshly built backend, performing real installs. Set `THALLIUM_STORE_FAKE_UNI=1` to simulate them instead. First launch shows a short setup walkthrough.

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

Launch it with the bundled wrapper (added to `PATH` by the package). It selects a frontend for you — see [Frontends](#frontends):

```bash
thallium-store
```

- **Home** — curated picks and collections.
- **Search** — click the search icon in the ribbon; every source is queried as you type, press Enter for full results.
- **Apps** — installed software and live operation activity (cancel a running job from here).
- **Updates** — available updates, refreshed on demand.
- **Settings** — cache storage controls, replay the first-run walkthrough, store info.

## Configuration

Set via environment variables (the wrappers in `scripts/` set sensible defaults):

| Variable | Default | Purpose |
| --- | --- | --- |
| `THALLIUM_STORE_FAKE_UNI` | `0` | `1` runs a safe simulator — progress and state without real package mutations. |
| `THALLIUM_STORE_BACKEND` | `thallium-store-backend` on `PATH` | Path to the backend binary. |
| `UNI_PRIVILEGE_BACKEND` | `pkexec` | Privilege escalation for apt/system flatpak mutations (`pkexec`, falling back to non-interactive `sudo -n`). |
| `THALLIUM_STORE_NATIVE` | autodetected | `native/qt6` only — path to the native Qt client, overriding the wrapper's search order. |
| `THALLIUM_NATIVE_BUILD_TYPE` | `Debug` | `native/qt6` only — `Release` builds under `target/native-release/`. |

State lives under `~/.local/share/thallium-store/` (SQLite DB, icon cache) and `~/.config/thallium-store/settings.json`.

## Architecture

The backend and the frontend talk newline-delimited JSON-RPC over a per-user Unix socket (or, for the web client, loopback HTTP). The backend is a Cargo workspace:

| Crate | Responsibility |
| --- | --- |
| `store-backend` | JSON-RPC daemon, socket server, request routing, caches, settings. |
| `store-catalog` | Source providers (apt, Flathub, GitHub, AppImage), merge/rank, detail enrichment. |
| `store-uni` | Install/remove/update backends (apt, flatpak, GitHub, AppImage), privilege escalation, progress streaming. Named for UNI, which it replaced. |
| `store-db` | SQLite operation history. |
| `store-core` | Shared models and the trust-ranking logic. |

`ui/shell.qml` is the QML frontend, in the Thallium 81 design language (Everforest palette, chamfered HUD surfaces). It is hosted either by Quickshell or by the native Qt 6 client in `native/` — the same file, two hosts. The web client in `web/` is a separate hand-written implementation of the same views over HTTP rather than the socket. Curated catalog data lives in `data/`.

## Contributing

```bash
cargo build          # build the workspace
cargo test           # run tests
./scripts/check      # fmt + clippy + tests
```

`./scripts/dev-run` is the fast loop; add `THALLIUM_STORE_FAKE_UNI=1` to
exercise install flows without mutating packages. Work on a frontend should not
need backend changes — if it does, the RPC contract in `docs/ipc.md` is the
thing to change, and all three frontends have to follow.

Heavy builds and CI run in GitHub Actions.

## License

[GNU General Public License v3.0](LICENSE) or later.
