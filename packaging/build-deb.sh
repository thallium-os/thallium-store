#!/usr/bin/env bash
# Build the `thallium-store` .deb and drop it where a publisher can pick it up.
#
# The store reaches installed machines through the same signed apt repo as the
# rest of Thallium, so the archive needs one command that yields a .deb at a
# known path. That is all this is: scripts/build-deb does the actual work and
# stays the single source of truth for what the package contains and what
# version it carries.
#
# This script used to live in the Thallium_81 monorepo and clone this repo at a
# PINNED_COMMIT, because the archive owned the decision of which store commit
# shipped. That pin is gone: a repo does not pin itself. The store now cuts its
# own .deb from its own checkout, and the archive publishes what it is handed.
# The delivery assertion moved with it -- the publisher checks the .deb it is
# about to publish is strictly newer than the version the published Packages
# index already advertises, which is the failure the pin comparison caught
# (0.5.25 published new content under the version already installed everywhere,
# and apt saw no upgrade).
#
# THALLIUM_STORE_TOKEN is likewise gone. There is no clone to authenticate.
set -euo pipefail

cd "$(dirname "$0")/.."

OUT_DIR="${1:-$PWD/dist}"
mkdir -p "$OUT_DIR"

echo ">> building"
bash scripts/build-deb

DEB=$(find dist -maxdepth 1 -name 'thallium-store_*.deb' -print -quit)
[ -n "$DEB" ] || { echo "::error::store build produced no .deb" >&2; exit 1; }

if [ "$(cd "$(dirname "$DEB")" && pwd)" != "$(cd "$OUT_DIR" && pwd)" ]; then
    install -m 0644 "$DEB" "$OUT_DIR/"
fi
echo ">> built $OUT_DIR/$(basename "$DEB")"
