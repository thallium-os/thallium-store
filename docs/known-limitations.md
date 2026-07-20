# Known Limitations

- Rust was not installed in the development container used for the initial scaffold, so local `cargo build` and `cargo test` could not be executed there.
- Real UNI package mutations are disabled by default. The local UNI script now has initial JSON event support, but it is a lifecycle wrapper around existing human commands, not deep backend progress.
- Updates, retry, cancellation of active jobs, and launching are contract stubs in this MVP.
- APT search uses `apt-cache search` as a read-only supplement. It is not a replacement for proper AppStream metadata.
- Flatpak search depends on the local Flatpak remote configuration.
- The bundled catalog is deliberately small and exists to make the vertical slice usable immediately.
- The QML client uses a backend `--request` helper and polling for queue updates. Direct persistent socket streaming should replace this after the core MVP is stable.
