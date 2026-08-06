# Ship Audit — 0.1.0

Pre-OTA audit of Thallium Store 0.1.0, run 2026-08-06 on branch
`mvp-integration` (from `817d90d`, ending at `ea7ff5a`).

Target platform, for the record: Thallium OS only — Debian 13 base, Hyprland,
quickshell. Every dependency judgement below assumes that baseline.

## Verdict

The store works. Ten issues were found; eight are fixed and verified, two are
open and listed under [Outstanding](#outstanding). Neither open item breaks
the build or the running app.

## Findings

| # | Severity | Finding | Status |
|---|----------|---------|--------|
| 1 | Blocker | Built `.deb` declared no dependency on `quickshell`, which `scripts/thallium-store` execs. Also listed `python3`, though the bundled UNI wrapper is bash. | Fixed — `6dfe6bb` |
| 2 | Blocker | `scripts/check` was red: `cargo fmt --check` failed, and clippy raised `manual checked division` (`appimage.rs`) and `type_complexity` (`store-catalog/src/lib.rs`). The gate never reached the test step. | Fixed — `f2def58`, `07ee28b` |
| 3 | Blocker | Two divergent packaging paths. `packaging/debian/rules` never installed `vendor/uni/uni`, used a different prefix, and declared a different dependency set than the `scripts/build-deb` path README actually ships. | Fixed — `6dfe6bb`, dead path deleted |
| 4 | High | `Icon=io.thallium.Store` in the desktop entry, with no icon anywhere in the tree. Launcher showed a blank tile. | Fixed — `6dfe6bb` |
| 5 | High | Anonymous GitHub API access is capped at 60 requests/hour/IP. Detail pages cost three requests each and an updates sweep costs one per installed GitHub app, so minutes of browsing blanked every GitHub panel until the hour rolled over. | Fixed — `07ee28b` |
| 6 | Medium | `docs/known-limitations.md` ships to `/usr/share/doc` and opened with a note about Rust missing from the initial scaffold container. It also called updates and cancellation stubs, which they no longer are. | Fixed — `ea7ff5a` |
| 7 | Medium | AppStream metainfo had no `<releases>`; update tooling keys off `<release version=…>`. | Fixed — `ea7ff5a` |
| 8 | Low | Version was maintained in three places (workspace `Cargo.toml`, `build-deb` default, debian changelog). | Fixed — `6dfe6bb`, now read from `Cargo.toml` |
| 9 | Low | `README.md` documented `sudo apt install ./thallium-store_*.deb`, but `build-deb` writes to `dist/`, so the glob matched nothing. It also advertised retry from the Apps view. | Fixed — `ea7ff5a` |
| 10 | Low | Both packaging scripts installed `ui/shell.qml` alone. Harmless while the UI is one file, but silently ships a UI that fails to import the moment it is split into `components/` or `pages/`. | Fixed — `6dfe6bb`, whole `ui/` tree packaged |

## Fixes

Four commits on `mvp-integration`:

- `07ee28b` — `fix(catalog): cache GitHub reads behind a rate-limit breaker`.
  All five GitHub call sites now route through one helper with a 15-minute
  response cache and a circuit breaker that stops issuing requests once GitHub
  reports the budget is spent, serving stale entries instead of blanks.
- `f2def58` — `style(uni): apply cargo fmt and use checked_div for progress`.
- `6dfe6bb` — `fix(packaging): ship a deb that can actually run`.
- `ea7ff5a` — `docs: correct the claims shipped to users`.

## Verification

`scripts/check` green end to end: `cargo fmt --check`, `cargo test`,
`cargo clippy --all-targets --all-features -- -D warnings`, `cargo build`.

`scripts/build-deb` produced `dist/thallium-store_0.1.0_amd64.deb` (3.1 MB).
Package contents confirmed by `dpkg-deb -c`: `quickshell` in `Depends`, icon at
`usr/share/icons/hicolor/scalable/apps/`, full `ui/` tree, version `0.1.0`
resolved from `Cargo.toml`.

Runtime checks were run against the **installed** prefix
(`~/.local/share/thallium-store` via `scripts/install-local`), not the dev tree:

| Surface | Result |
|---------|--------|
| `system.health` | `ready`; `fakeUni:false`; apt, flatpak, privilege all true |
| `catalog.search "gimp"` | 1.0s; all four providers `ready` |
| `catalog.discover` | 4 collections populated |
| `catalog.appDetails` (Flathub) | icon cached locally, screenshots, license resolved |
| `catalog.appDetails` (GitHub) | enriched; 2.03s cold, 0.05s warm — cache confirmed |
| `installed.list` / `updates.list` | 6 / 8 items |
| GUI | window mapped, no QML errors, live backend requests |

## Outstanding

- **`operations.retry` and `apps.launch` return errors.** Both are unimplemented
  and now documented as such rather than built — that is feature work, not a
  ship fix. `operations.cancel` is implemented and working.
- **Hero card layout glitch.** On the app detail page, a stray dark rectangle
  overlaps the **Get** button and the `FREE` label in the top-right of the hero
  card. Cosmetic, appears on every detail page.

## Not covered by this audit

- The `.deb` was validated by contents and metadata, not by a real
  `sudo apt install` on a clean machine. Worth one manual install test before
  publishing.
- No CI exists (`.github/` is absent); `scripts/check` is manual.
- No OTA publishing mechanism was found in this repo or elsewhere locally, so
  the distribution step itself is unaudited.
