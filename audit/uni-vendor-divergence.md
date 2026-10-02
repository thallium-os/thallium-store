# D-12 — `thallium-store/vendor/uni/uni` vs `uni` upstream

Date: 2026-08-08
Mode: report only. Neither copy modified, nothing merged, nothing pushed.

> **Brief note.** `~/Projects/thallium-release-brief.md` revision 5 does not
> exist on disk — `~/Projects/` has no files at top level, and no `*brief*`
> file exists anywhere under `~`. §7 / D-11..D-15 could not be read. This
> report follows the direction stated in the prompt: **the vendored copy is
> live and newer; public `uni` is older.** That direction is confirmed
> independently by the evidence in §4 below. D-13 remains unread.

## 0. Correction to the earlier uni audit

`audit/uni-pre-public-report.md` (2026-08-07) contains two errors, both now
corrected by measurement:

| Claim | Correct |
|---|---|
| U-4: "branch `0.6.0` is 2 commits **behind** `master`" | `0.6.0` is 2 commits **ahead** of `master`. `git rev-list --left-right --count master...0.6.0` → `0  2`; left=only-in-master=0. I read the columns backwards. |
| U-2: vendored copy is "a fork ... to discard" in favour of upstream | Reversed. The vendored copy carries the load-bearing code (§4). Upstream `master` is the stale one. |

The GPL-3.0 §5 obligation noted in U-2 still stands — a modified copy shipped
in `thallium-store` needs a modification notice and a third-party record. That
part was correct; only the direction was wrong.

## 1. Topology

```
master (origin/HEAD, 1743 lines)  ──┬── 0.6.0 branch  (2132 lines, +2 commits)
        head 1b74fd7               │      dd173f9 CLI additions (index/search/sources/info, --json)
                                   │      ec91955 uni-ui Qt6 frontend + packaging
                                   │
                                   └── vendored copy (2324 lines, not in any git)
```

Divergence base identified by diffing the vendored copy against every
historical revision of `uni` in the upstream repo:

| diff lines | revision | |
|---|---|---|
| **712** | `1b74fd7` | **closest — `master` HEAD, the base** |
| 734 | `844a9329` | Reset version scheme 2.x → 0.x |
| 1142 | `dd173f9` | 0.6.0 CLI additions |
| 1143 | — | vendored vs `0.6.0` branch head |

**The vendored copy forked from `master`, not from `0.6.0`.** Since `1b74fd7`
*is* `master` HEAD, `master` has not moved since the fork — so there is no
conflict against `master` at all. The entire conflict surface is against the
`0.6.0` branch, which advanced independently.

Function counts: base `master` 75, vendored 86, `0.6.0` 84.

## 2. What the vendored copy ADDS

11 new functions, none present upstream on any branch.

### Privilege escalation (the load-bearing part)

| Function | Purpose |
|---|---|
| `privilege_runner` | Chooses `pkexec` or `sudo`. Honours `UNI_PRIVILEGE_BACKEND` (`pkexec`/`sudo`/`auto`); `auto` picks `pkexec` when stdin is not a TTY and `WAYLAND_DISPLAY`/`DISPLAY` is set. |
| `run_privileged` | Wraps a command through the chosen runner; resolves to an absolute path first, because `pkexec` requires one. |

Upstream `master` and `0.6.0` call bare `sudo` at 10+ sites. `sudo` from a GUI
process with no controlling terminal cannot prompt — it fails. This is the code
that makes `uni` usable from a desktop app at all.

### JSON event stream and machine-readable commands

| Function | Purpose |
|---|---|
| `json_event` | Emits `{schemaVersion:1, event, state, progress, message, timestamp, package, source, exitCode}`; strips ANSI from `message`. |
| `cmd_json_events_wrapper` | Drives a command while emitting the above. |
| `cmd_search_json`, `cmd_info_json`, `cmd_installed_json`, `cmd_updates_json` | JSON forms of the four read commands. |

### Backend additions

| Function | Purpose |
|---|---|
| `install_appimage_from_github_spec` | Install an AppImage direct from a GitHub release spec. |
| `pick_matching_asset` | Asset selection for the above. |
| `apt_fix_cmd` | Wraps `apt -f install` repair. |

Plus vendored-only edits to 10 existing functions: `apt_cmd`, `cmd_purge`,
`cmd_remove`, `cmd_update`, `do_apt_install_many`, `do_install`, `install_deb`,
`print_update_banner`, `registry_list`, `single_source_search` — consistent
with routing every privileged call through `run_privileged`.

## 3. What the vendored copy REMOVES

**Nothing.** Zero functions from base `master` are absent from the vendored
copy. The divergence is purely additive against its base.

Note the asymmetry this creates: the vendored copy does not remove anything
from `master`, but it also never received the `0.6.0` branch work, so relative
to the *newest* upstream code it is missing 9 functions — see §5.

## 4. Evidence the vendored copy is the live one

Not inference — `thallium-store` hard-depends on vendored-only code:

```
scripts/thallium-store:33  export UNI_PRIVILEGE_BACKEND="${UNI_PRIVILEGE_BACKEND:-pkexec}"
scripts/dev-run:17         export UNI_PRIVILEGE_BACKEND="${UNI_PRIVILEGE_BACKEND:-pkexec}"
scripts/dev-run:19         export THALLIUM_STORE_UNI="${THALLIUM_STORE_UNI:-$PWD/vendor/uni/uni}"
```

`UNI_PRIVILEGE_BACKEND` exists **only** in the vendored copy. Point the store at
upstream `master` or `0.6.0` and the variable is ignored, escalation falls back
to `sudo`, and every privileged operation fails silently in a GUI session.

The consuming struct matches the vendored schema, not `0.6.0`'s:

```rust
// crates/store-uni/src/lib.rs:20
pub struct UniProgress {
    pub state: OperationState,   // vendored json_event has "state"
    pub percent: u8,
    pub message: String,         // vendored json_event has "message"
}
```

`0.6.0`'s `emit_event` emits `{package, stage, …}` — no `state` field. Feeding
it to `UniProgress` fails deserialization.

## 5. Conflict surface

### 5a. Semantic collisions — same job, different name and schema

The dangerous category. These will not conflict textually and will not be
caught by a merge tool.

| Vendored | `0.6.0` | Collision |
|---|---|---|
| `json_event` | `emit_event` | **Two incompatible JSON event schemas.** Vendored: `{schemaVersion:1, event, state, progress, message, timestamp, package, source, exitCode}`, always emits, ANSI-stripped. `0.6.0`: `{package, stage, k=v…}`, gated on `UNI_JSON`, swallows errors (`2>/dev/null \|\| true`). Only the vendored schema has a consumer. |
| `cmd_search_json` | `cmd_search` | Both machine-readable search; different output shapes. |
| `cmd_info_json` | `cmd_info` | Same overlap. |

### 5b. Functions modified in BOTH since base — true three-way conflicts

4 functions, all confirmed non-identical between the two heads:

| Function | base | vendored | 0.6.0 | Nature |
|---|---|---|---|---|
| `main` | 26 L | **53 L** | 30 L | Worst. Vendored more than doubles the dispatcher for the `*_json` commands; `0.6.0` adds `index`/`search`/`sources`/`info`. Both rewrite the same `case` block. |
| `cmd_install_one` | 134 L | 146 L | 145 L | Vendored routes through `run_privileged`; `0.6.0` adds `emit_event` calls. Both edit the same install path. |
| `cmd_help` | 31 L | 37 L | 36 L | Each documents its own new commands. Mechanical. |
| `cmd_install` | 83 L | 83 L | 87 L | Vendored edits in place (same length, different content); `0.6.0` adds event emission. |

### 5c. Clean — modified on one side only

No conflict; these merge mechanically.

- **Vendored only (10):** `apt_cmd`, `cmd_purge`, `cmd_remove`, `cmd_update`,
  `do_apt_install_many`, `do_install`, `install_deb`, `print_update_banner`,
  `registry_list`, `single_source_search`
- **`0.6.0` only (8):** `cmd_self_update`, `die`, `divider`, `header`, `info`,
  `step`, `success`, `warn`

Note `cmd_self_update` is in the `0.6.0`-only column — the function D-11 covers
has been edited upstream but not in the vendored copy. Per instruction, no
action taken on it here.

### 5d. Additions that do not overlap at all

`0.6.0`'s 9 new functions — `cmd_index`, `_index_apt`, `_index_flatpak`,
`_ensure_index_schema`, `_search_cached`, `cmd_sources`, plus `cmd_search`,
`cmd_info`, `emit_event` — implement the SQLite FTS5 cached index and the
source benchmark. Apart from the three collisions in §5a, this body of work is
orthogonal to everything the vendored copy added and is exactly what the store
would gain from a reconciliation.

## 6. Summary for review

- Base is `master` HEAD `1b74fd7`. Vendored diverged from `master`; `master`
  has not moved since. `0.6.0` advanced 2 commits independently.
- Vendored adds 11 functions, removes 0, edits 10 shared ones. It is
  additive against its base.
- The store cannot run on upstream: `UNI_PRIVILEGE_BACKEND` and the
  `state`/`message` event schema are vendored-only and are consumed by
  `scripts/thallium-store` and `UniProgress` respectively.
- Real conflict surface is small — **4 functions** (`main`, `cmd_install_one`,
  `cmd_help`, `cmd_install`) plus **3 semantic collisions** (`json_event` vs
  `emit_event`, and two `*_json` command pairs). Everything else is disjoint.
- `main` is the hard one: both sides rewrote the same dispatch `case`.

No merge performed. No file modified. No push.
