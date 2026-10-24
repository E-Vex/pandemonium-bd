#!/usr/bin/env bash
# A-014: fetch the official asset packs into the dev layout.
#
# The official art lives in its own repo, E-Vex/pandemonium-assets, and is
# NEVER committed to this repository (docs/ASSETS.md). This script clones or
# updates that repo under crates/client/assets/packs/ (gitignored) and copies
# its default/ pack to crates/client/assets/packs/default/ — the compile-time
# dev path the client discovers when no --pack flag is given (pack_dir_from,
# crates/client/src/main.rs).
#
# One command, idempotent, safe to re-run:
#   scripts/fetch-assets.sh
#
# The URL can be overridden for forks (a checkout of the assets repo next to
# a fork of the game, for example):
#   PANDEMONIUM_ASSETS_REPO=<url> scripts/fetch-assets.sh
#
# A missing pack is never an error for the game itself: the client falls back
# to placeholder rendering (docs/ASSETS.md). This script exists so the pack
# is one command away, not a build requirement.

set -euo pipefail

REPO_URL="${PANDEMONIUM_ASSETS_REPO:-https://github.com/E-Vex/pandemonium-assets.git}"
REF="${PANDEMONIUM_ASSETS_REF:-master}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PACKS_DIR="$ROOT/crates/client/assets/packs"
CLONE_DIR="$PACKS_DIR/pandemonium-assets"
PACK_DIR="$PACKS_DIR/default"

command -v git >/dev/null 2>&1 \
  || { echo "fetch-assets: git is required (the repo you cloned needed it too)"; exit 1; }

mkdir -p "$PACKS_DIR"

# Clone shallow on first use; update the existing shallow clone afterwards.
# reset --hard FETCH_HEAD, not pull: the packs directory is gitignored local
# state, so any local drift there is disposable — the remote is the truth.
if [ -d "$CLONE_DIR/.git" ]; then
  echo "fetch-assets: updating $REPO_URL ($REF)"
  git -C "$CLONE_DIR" fetch --depth 1 origin "$REF"
  git -C "$CLONE_DIR" reset --hard FETCH_HEAD
else
  echo "fetch-assets: cloning $REPO_URL ($REF, shallow)"
  rm -rf "$CLONE_DIR"
  git clone --depth 1 --branch "$REF" "$REPO_URL" "$CLONE_DIR"
fi

[ -f "$CLONE_DIR/default/manifest.ron" ] \
  || { echo "fetch-assets: $REPO_URL has no default/manifest.ron — nothing to install"; exit 1; }

rm -rf "$PACK_DIR"
cp -R "$CLONE_DIR/default" "$PACK_DIR"

PACK_ID="$(sed -n 's/^.*pack_id:[[:space:]]*"\(.*\)".*$/\1/p' "$PACK_DIR/manifest.ron" | head -1)"
FILES="$(find "$PACK_DIR" -type f | wc -l | tr -d ' ')"
echo "fetch-assets: pack 'default' (pack_id: ${PACK_ID:-?}) installed at $PACK_DIR ($FILES files)"
echo "fetch-assets: run 'cargo run -p pandemonium-client' — it discovers this pack at startup"
