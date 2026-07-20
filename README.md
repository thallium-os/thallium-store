# Thallium Store MVP

Thallium Store is a Quickshell/QML app store frontend for Thallium OS. The backend is Rust and speaks newline-delimited JSON-RPC over a per-user Unix socket. UNI remains the install authority.

The packaged app defaults to `THALLIUM_STORE_FAKE_UNI=0` and uses the bundled JSON-capable UNI backend. Set `THALLIUM_STORE_FAKE_UNI=1` only when you want safe demo progress without real package mutations.

## Prerequisites

- Debian/Thallium OS baseline
- Rust toolchain with Cargo
- Quickshell
- UNI
- Flatpak for Flathub search
- `apt-cache` for system package search

## Build

```bash
cargo build
```

## Run Backend

```bash
THALLIUM_STORE_FAKE_UNI=1 cargo run -p thallium-store-backend
```

The socket is created at:

```text
$XDG_RUNTIME_DIR/thallium-store/backend.sock
```

## Run UI

Recommended developer launcher:

```bash
scripts/dev-run
```

Install into your user account:

```bash
scripts/install-local
thallium-store
```

Build a Debian package:

```bash
scripts/build-deb
sudo dpkg -i dist/thallium-store_0.1.0_*.deb
```

Manual equivalent:

```bash
PATH="$PWD/target/debug:$PATH" THALLIUM_STORE_FAKE_UNI=1 quickshell --path ui/shell.qml
```

The UI starts `thallium-store-backend` from `PATH`.

## Check

```bash
scripts/check
```

## Test IPC

```bash
cargo run -p thallium-store-backend -- --request system.health '{}'
cargo run -p thallium-store-backend -- --request catalog.search '{"query":"gimp","sources":["system","flathub","github"],"limit":20}'
```

## Test

```bash
cargo fmt --check
cargo test
```

## Architecture

```text
Quickshell/QML
  -> thallium-store-backend JSON-RPC client helper
  -> Unix socket backend
  -> catalog providers, SQLite queue, UNI adapter
  -> UNI CLI
```

The backend stores durable queue state at:

```text
$XDG_DATA_HOME/thallium-store/store.db
```

## Current MVP Features

- Bundled starter app catalog for GIMP, OBS Studio, and Zed.
- Read-only Flatpak search using `flatpak search`.
- Read-only system search using `apt-cache search`.
- Source ranking and basic deduplication.
- Quickshell search, details, queue, and install actions.
- JSON-RPC IPC over a local Unix socket.
- SQLite operation persistence and logs.
- Bounded fake operation scheduler with source-specific semaphores.
- Fake UNI progress stream for safe demos.

## UNI JSON Contract Status

The local UNI script has been patched with initial support for:

```bash
uni search <query> --json
uni info <id> --source <source> --json
uni installed --json
uni updates --json
uni install <id> --source <source> --json-events
uni remove <id> --source <source> --json-events
uni update <id> --source <source> --json-events
```

The `--json-events` path is currently a lifecycle wrapper around the existing human commands. It emits valid newline-delimited JSON and captures command logs as JSON log events. Rich backend-specific progress percentages still need deeper UNI integration.

Expected long-term upstream contract:

```bash
uni search <query> --json
uni info <id> --source <source> --json
uni installed --json
uni updates --json
uni install <id> --source <source> --json-events
uni remove <id> --source <source> --json-events
uni update <id> --source <source> --json-events
```

The store adapter in `crates/store-uni` already parses JSON event lines. To try real package operations from the GUI, run with:

```bash
THALLIUM_STORE_FAKE_UNI=0 thallium-store
```

Only do that from a disposable VM until real install/remove flows have been tested carefully.
