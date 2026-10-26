#!/usr/bin/env bash
# A-018 — build the D-002 pilot pack (M12 phases 3-4, ADR-0101 §4): a
# schema-2 asset pack, OUTSIDE this repo, that puts the Designer's three
# staged D-002 models on their kinds with the official pack's skins as the
# fallback tier:
#
#   rifleman  ← design/staging/d002/models/units/rifleman.glb     (+ skin)
#   barracks  ← design/staging/d002/models/buildings/barracks.glb (+ skin)
#   ore_node  ← design/staging/d002/models/nodes/ore_node.glb     (model
#               only — the official pack ships no ore_node skin, so the
#               placeholder silhouette is that kind's fallback tier)
#
# The OFFICIAL pack (pandemonium-assets/default) is never modified and stays
# schema 1; this script only reads it (skins, terrain) and the staging tree.
#
# Usage:
#   scripts/make-pilot-pack.sh                 # all three models
#   scripts/make-pilot-pack.sh rifleman        # a one-model pack (the
#                                              # machine pass's shape)
#   scripts/make-pilot-pack.sh rifleman barracks
#
# Then look at it:
#   cargo run -p pandemonium-client -- --pack /tmp/pandemonium-pilot-pack
#
# Environment knobs:
#   PILOT_OUT                  target directory
#                              (default: /tmp/pandemonium-pilot-pack)
#   PANDEMONIUM_ASSETS_REPO    the assets repo (default: the official URL)
#   PANDEMONIUM_ASSETS_REF     ref to check out (default: master)
#
# Idempotent: re-running rebuilds the pack from the (updated) clone. The
# clone lives under the system temp dir, never inside this repository.

set -euo pipefail

REPO_URL="${PANDEMONIUM_ASSETS_REPO:-https://github.com/E-Vex/pandemonium-assets.git}"
REF="${PANDEMONIUM_ASSETS_REF:-master}"
OUT_DIR="${PILOT_OUT:-/tmp/pandemonium-pilot-pack}"

# The three staged D-002 models: kind → (path inside the assets repo, pack
# subdirectory). The official pack has no ore_node skin — its fallback tier
# is the placeholder silhouette, which is exactly the chain's third tier.
declare -A D002_MODELS=(
    [rifleman]="design/staging/d002/models/units/rifleman.glb"
    [barracks]="design/staging/d002/models/buildings/barracks.glb"
    [ore_node]="design/staging/d002/models/nodes/ore_node.glb"
)
declare -A D002_SUBDIR=(
    [rifleman]=units
    [barracks]=buildings
    [ore_node]=nodes
)
# Every kind the official (schema-1) pack skins — the fallback tier — in
# manifest order, with ore_node slotted in (model-only).
ALL_KINDS=(barracks command_center guardian ore_node raider rifleman supply_depot turret worker)
TERRAIN_CLASSES=(grass gravel rock sand)

# Which kinds carry a model in this pack: the arguments, default all three.
REQUESTED=("$@")
if [ "${#REQUESTED[@]}" -eq 0 ]; then
    REQUESTED=(rifleman barracks ore_node)
fi
for kind in "${REQUESTED[@]}"; do
    [ -n "${D002_MODELS[$kind]:-}" ] || {
        echo "make-pilot-pack: unknown model kind '$kind' (known: rifleman barracks ore_node)"
        exit 1
    }
done

command -v git >/dev/null 2>&1 \
    || { echo "make-pilot-pack: git is required"; exit 1; }

# ---------------------------------------------------------------------------
# 1. The assets repo: a shallow clone under the temp dir, updated in place
#    (the same never-inside-this-repo rule fetch-assets.sh follows; this
#    script additionally never touches the dev pack layout).
# ---------------------------------------------------------------------------
CLONE_DIR="${TMPDIR:-/tmp}/pandemonium-pilot-assets"
if [ -d "$CLONE_DIR/.git" ]; then
    echo "make-pilot-pack: updating $REPO_URL ($REF) at $CLONE_DIR"
    git -C "$CLONE_DIR" fetch --depth 1 origin "$REF" >/dev/null
    git -C "$CLONE_DIR" reset --hard FETCH_HEAD >/dev/null
else
    echo "make-pilot-pack: cloning $REPO_URL ($REF, shallow) to $CLONE_DIR"
    rm -rf "$CLONE_DIR"
    git clone --depth 1 --branch "$REF" "$REPO_URL" "$CLONE_DIR" >/dev/null
fi

for kind in "${REQUESTED[@]}"; do
    [ -f "$CLONE_DIR/${D002_MODELS[$kind]}" ] || {
        echo "make-pilot-pack: $CLONE_DIR/${D002_MODELS[$kind]} does not exist — is $REF carrying the D-002 staging models?"
        exit 1
    }
done
[ -f "$CLONE_DIR/default/manifest.ron" ] || {
    echo "make-pilot-pack: $REPO_URL has no default/manifest.ron — no skins to fall back to"
    exit 1
}

# ---------------------------------------------------------------------------
# 2. Assemble the pack: one pass per kind — the model when requested (and
#    its skin when the official pack ships one), else the skin alone; then
#    the terrain classes, verbatim from the official pack.
# ---------------------------------------------------------------------------
rm -rf "$OUT_DIR"
mkdir -p "$OUT_DIR/models/units" "$OUT_DIR/models/buildings" "$OUT_DIR/models/nodes" \
    "$OUT_DIR/textures/entities" "$OUT_DIR/textures/terrain"

MANIFEST="$OUT_DIR/manifest.ron"
{
    printf '(\n'
    printf '    pack_id: "pilot_d002",\n'
    printf '    schema_version: 2,\n'
    printf '    entities: {\n'
} > "$MANIFEST"
MODELS=0
SKINS=0
for kind in "${ALL_KINDS[@]}"; do
    has_model=0
    case " ${REQUESTED[*]} " in
        *" $kind "*) has_model=1 ;;
    esac
    has_skin=0
    [ -f "$CLONE_DIR/default/textures/entities/$kind.png" ] && has_skin=1
    if [ "$has_model" = 0 ] && [ "$has_skin" = 0 ]; then
        continue
    fi
    printf '        "%s": (\n' "$kind" >> "$MANIFEST"
    if [ "$has_model" = 1 ]; then
        subdir="${D002_SUBDIR[$kind]}"
        cp "$CLONE_DIR/${D002_MODELS[$kind]}" "$OUT_DIR/models/$subdir/$kind.glb"
        printf '            model: Some("models/%s/%s.glb"),\n' "$subdir" "$kind" >> "$MANIFEST"
        MODELS=$((MODELS + 1))
    fi
    if [ "$has_skin" = 1 ]; then
        cp "$CLONE_DIR/default/textures/entities/$kind.png" \
            "$OUT_DIR/textures/entities/$kind.png"
        printf '            skin: Some("textures/entities/%s.png"),\n' "$kind" >> "$MANIFEST"
        SKINS=$((SKINS + 1))
    fi
    printf '        ),\n' >> "$MANIFEST"
done
{
    printf '    },\n'
    printf '    terrain: {\n'
} >> "$MANIFEST"
for class in "${TERRAIN_CLASSES[@]}"; do
    src="$CLONE_DIR/default/textures/terrain/$class.png"
    [ -f "$src" ] || { echo "make-pilot-pack: official terrain texture missing: $src"; exit 1; }
    cp "$src" "$OUT_DIR/textures/terrain/$class.png"
    printf '        "%s": "textures/terrain/%s.png",\n' "$class" "$class" >> "$MANIFEST"
done
printf '    },\n)\n' >> "$MANIFEST"

ENTRIES=$((MODELS + SKINS))
SKINNED_ONLY=$((SKINS))
echo "make-pilot-pack: schema-2 pilot pack at $OUT_DIR"
echo "make-pilot-pack: $MODELS model entr$( [ "$MODELS" = 1 ] && echo y || echo ies ) ($(printf '%s ' "${REQUESTED[@]}")), $SKINS skin fallbacks total, 4 terrain classes"
echo "make-pilot-pack: try it — cargo run -p pandemonium-client -- --pack $OUT_DIR"
