#!/usr/bin/env bash
# Build the `thallium-store` .deb from a pinned commit of dronzer-tb/thallium-store.
#
# The store lives in its own repo (Rust workspace + QML), but it has to reach
# installed machines through the same signed apt repo as the rest of Thallium,
# so it is built here and dropped into pool/main alongside the thallium-*
# packages. Same shape as packaging/auto-cpufreq/build-deb.sh.
#
# The pin is a full commit SHA, not a branch: this runs in CI and its output
# goes into a GPG-signed repo, so "whatever mvp-integration happened to be at"
# is not an acceptable input. Bump PINNED_COMMIT to ship a new store.
#
# The repo is private, so the clone needs a token with read access to it in
# THALLIUM_STORE_TOKEN. Without it this exits non-zero rather than shipping a
# release that silently lacks the store.
set -euo pipefail

STORE_REPO=dronzer-tb/thallium-store
PINNED_COMMIT=85e0426ea7177a1e1c97bb5e7c8ee67283c53b6b

OUT_DIR="${1:-$PWD}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

if [ -z "${THALLIUM_STORE_TOKEN:-}" ]; then
    echo "::error::THALLIUM_STORE_TOKEN is not set; cannot clone $STORE_REPO" >&2
    exit 1
fi

echo ">> cloning $STORE_REPO at ${PINNED_COMMIT:0:12}"
SRC="$WORK/thallium-store"
git init -q "$SRC"
git -C "$SRC" remote add origin \
    "https://x-access-token:${THALLIUM_STORE_TOKEN}@github.com/${STORE_REPO}.git"
git -C "$SRC" fetch -q --depth 1 origin "$PINNED_COMMIT"
git -C "$SRC" checkout -q FETCH_HEAD

# The store's own scripts/build-deb is the single source of truth for what the
# package contains and what version it carries (read from its Cargo.toml).
# Duplicating it here would mean the deb this repo ships drifts from the one
# the store's authors test with.
echo ">> building"
( cd "$SRC" && bash scripts/build-deb )

DEB=$(find "$SRC/dist" -maxdepth 1 -name 'thallium-store_*.deb' -print -quit)
[ -n "$DEB" ] || { echo "::error::store build produced no .deb" >&2; exit 1; }

install -m 0644 "$DEB" "$OUT_DIR/"
echo ">> built $OUT_DIR/$(basename "$DEB")"
