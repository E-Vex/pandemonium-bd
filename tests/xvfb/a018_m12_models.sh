#!/usr/bin/env bash
# A-018 — Xvfb + XTEST machine verification of the M12 model pipeline
# (phases 3-4: the renderer pass + the fallback chain, ADR-0101).
#
# Verifies, against the real windowed client on the real pilot pack:
#
#   c0  the environment gate: the pilot pack loads (schema 2) and all three
#       D-002 models load with their evidence lines — 900 frames present
#   c1  the one-model pack: exactly one model line (rifleman), the other
#       kinds on their skins — 300 frames present
#   c2  the A-146 pin, live: a corrupted rifleman.glb + its valid skin, a
#       full --frames 900 windowed run greps EXACTLY ONE occurrence of the
#       rifleman diagnostic; the skin still loads; the other kinds' lines
#       are untouched; zero per-frame logging (one occurrence across 900
#       frames is the proof)
#   c3  the placeholder tier: a corrupted ore_node.glb with no skin declared
#       — one diagnostic, the placeholder tier named, 300 frames present
#   c4  the frame timeline: an AI-vs-AI spectate match on the pilot pack at
#       the default camera captures dated frames to the evidence dir (the
#       report's three deliverable frames — rifleman group, barracks, ore
#       node — are selected from this timeline; the *look* needs owner eyes
#       per STANDING_ORDERS §8, the machine proves the frames are live
#       renders, not void)
#
# The recipe is AI-Handoff.md §9 (DEBT-008), the a013 structure: Xvfb run
# directly (no xvfb-run, no window manager), keyboard focus via python-xlib
# set_input_focus, input via XTEST, frame capture via get_image, graceful
# close via the WM_DELETE_WINDOW ClientMessage.
#
# Re-runnable: `bash tests/xvfb/a018_m12_models.sh [check ...]`
# (no arguments = all checks). Environment knobs:
#   A018_EVIDENCE_DIR  where the per-check logs and frames go
#                      (default: <repo>/target/a018-evidence)
#   A018_USERLAND      the userland libEGL/libXtst tree (default:
#                      $HOME/.local/share/pandemonium/userland-libs;
#                      bootstrapped via apt-get download + dpkg -x if
#                      missing — no root needed)
#   A018_DISPLAY       the Xvfb display number (default 95)
#   A018_ASSETS_REPO   override for the assets repo URL (default: the
#                      official one; a local file:// clone works too)
#
# Never runs the client outside Xvfb. Never modifies source files (the
# pilot packs are built OUTSIDE the repository, per the brief).

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${A018_EVIDENCE_DIR:-$REPO_ROOT/target/a018-evidence}"
USERLAND="${A018_USERLAND:-$HOME/.local/share/pandemonium/userland-libs}"
DISPLAY_NUM="${A018_DISPLAY:-95}"
ASSETS_REPO="${A018_ASSETS_REPO:-}"
PILOT_DIR="${A018_PILOT_OUT:-/tmp/pandemonium-a018-pilot}"
PILOT_ONE="${A018_PILOT_ONE_OUT:-/tmp/pandemonium-a018-one-model}"
PILOT_BAD="${A018_PILOT_BAD_OUT:-/tmp/pandemonium-a018-bad-model}"
PILOT_BAD_NODE="${A018_PILOT_BAD_NODE_OUT:-/tmp/pandemonium-a018-bad-node}"
mkdir -p "$EVIDENCE_DIR"

log()  { printf '%s\n' "$*"; }

# ---------------------------------------------------------------------------
# 1. Hard environment requirements (AI-Handoff.md §9).
# ---------------------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
for tool in Xvfb python3 cargo; do
    command -v "$tool" >/dev/null 2>&1 || {
        log "A-018 BLOCKER: '$tool' is not installed and cannot be substituted."
        exit 2
    }
done
if ! python3 -c "import Xlib" >/dev/null 2>&1; then
    python3 -m pip install python-xlib >/dev/null 2>&1 \
        || python3 -m pip install --break-system-packages python-xlib >/dev/null 2>&1 \
        || { log "A-018 BLOCKER: python-xlib is not importable and pip cannot install it."; exit 2; }
fi

# ---------------------------------------------------------------------------
# 2. The userland library tree (no root), the a013 recipe.
# ---------------------------------------------------------------------------
needs_userland() {
    ldconfig -p 2>/dev/null | grep -q "libEGL.so.1" || return 0
    ldconfig -p 2>/dev/null | grep -q "libXtst.so.6" || return 0
    ldconfig -p 2>/dev/null | grep -q "libxkbcommon-x11.so.0" || return 0
    return 1
}
if [ -d "$USERLAND/usr/lib/x86_64-linux-gnu" ]; then
    log "A-018 env: userland library tree at $USERLAND (pre-existing)"
elif needs_userland; then
    log "A-018 env: bootstrapping the userland library tree at $USERLAND ..."
    DEB_DIR="$(mktemp -d)"
    (
        cd "$DEB_DIR" || exit 1
        PACKS="libegl1 libegl-mesa0 libgles2 libxtst6 libxkbcommon-x11-0 libxcb-xkb1"
        if ! pkg-config --exists alsa; then
            PACKS="$PACKS libasound2-dev"
            ldconfig -p 2>/dev/null | grep -q "libasound.so.2" || PACKS="$PACKS libasound2t64"
        fi
        ldconfig -p 2>/dev/null | grep -q "libgallium" || PACKS="$PACKS mesa-libgallium"
        # shellcheck disable=SC2086
        apt-get download $PACKS >/dev/null 2>&1 || { echo "apt-get download failed"; exit 1; }
        mkdir -p "$USERLAND"
        for deb in *.deb; do dpkg -x "$deb" "$USERLAND"; done
    ) || { log "A-018 BLOCKER: the userland library bootstrap failed."; exit 2; }
    rm -rf "$DEB_DIR"
    LIBD="$USERLAND/usr/lib/x86_64-linux-gnu"
    [ -e "$LIBD/libxkbcommon.so" ]     || cp "$LIBD/libxkbcommon.so.0" "$LIBD/libxkbcommon.so"
    [ -e "$LIBD/libxkbcommon-x11.so" ] || cp "$LIBD/libxkbcommon-x11.so.0" "$LIBD/libxkbcommon-x11.so"
    mkdir -p "$USERLAND/glvnd/egl_vendor.d"
    printf '%s\n' \
        '{' \
        '    "file_format_version" : "1.0.0",' \
        '    "ICD" : {' \
        '        "library_path" : "libEGL_mesa.so.0"' \
        '    }' \
        '}' > "$USERLAND/glvnd/egl_vendor.d/50_mesa.json"
    if [ -f "$LIBD/pkgconfig/alsa.pc" ]; then
        [ -e "$LIBD/libasound.so.2.0.0" ] || cp /usr/lib/x86_64-linux-gnu/libasound.so.2.0.0 "$LIBD/" 2>/dev/null || true
        sed -i "s|^prefix=.*|prefix=$USERLAND/usr|" "$LIBD/pkgconfig/alsa.pc"
    fi
    log "A-018 env: userland library tree bootstrapped at $USERLAND"
else
    log "A-018 env: system EGL/XTEST libraries present; userland tree not needed"
    USERLAND=""
fi

if [ -n "$USERLAND" ]; then
    export LD_LIBRARY_PATH="$USERLAND/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LIBRARY_PATH="$USERLAND/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
    [ -f "$USERLAND/usr/lib/x86_64-linux-gnu/pkgconfig/alsa.pc" ] \
        && export PKG_CONFIG_PATH="$USERLAND/usr/lib/x86_64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    export __EGL_VENDOR_LIBRARY_DIRS="$USERLAND/glvnd/egl_vendor.d"
fi

export WGPU_BACKEND=gl
export LIBGL_ALWAYS_SOFTWARE=1
export DISPLAY=":$DISPLAY_NUM"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/xdg-runtime}"
mkdir -p "$XDG_RUNTIME_DIR" 2>/dev/null || true

{
    echo "A-018 environment ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
    echo "Xvfb: $(command -v Xvfb)$(dpkg -l xvfb 2>/dev/null | awk '/^ii/ {print " ("$3")"}')"
    echo "python: $(python3 --version 2>&1), python-xlib: $(python3 -c 'import Xlib; print(Xlib.__version__)' 2>&1)"
    echo "XTEST injection: python-xlib xtest_fake_input (keys) + get_image (frames)"
    echo "renderer: WGPU_BACKEND=$WGPU_BACKEND LIBGL_ALWAYS_SOFTWARE=$LIBGL_ALWAYS_SOFTWARE (Mesa llvmpipe)"
    echo "display: $DISPLAY, evidence: $EVIDENCE_DIR"
    echo
} | tee "$EVIDENCE_DIR/env.txt"

# ---------------------------------------------------------------------------
# 3. Build the client (release) and the pilot pack variants.
# ---------------------------------------------------------------------------
log "A-018 build: cargo build -p pandemonium-client --release"
if ! (cd "$REPO_ROOT" && cargo build -p pandemonium-client --release 2>&1 | tail -2); then
    log "A-018 BLOCKER: the client failed to build."
    exit 2
fi
BIN="$REPO_ROOT/target/release/pandemonium-client"
[ -x "$BIN" ] || { log "A-018 BLOCKER: $BIN missing after the build."; exit 2; }

log "A-018 packs: building the pilot pack variants (outside the repository)"
MAKE_PACK=(bash "$REPO_ROOT/scripts/make-pilot-pack.sh")
[ -n "$ASSETS_REPO" ] && MAKE_PACK=(env PANDEMONIUM_ASSETS_REPO="$ASSETS_REPO" "${MAKE_PACK[@]}")
PILOT_OUT="$PILOT_DIR" "${MAKE_PACK[@]}" >/dev/null 2>&1 \
    || { log "A-018 BLOCKER: make-pilot-pack.sh failed (the three-model pack)."; exit 2; }
PILOT_OUT="$PILOT_ONE" "${MAKE_PACK[@]}" rifleman >/dev/null 2>&1 \
    || { log "A-018 BLOCKER: make-pilot-pack.sh rifleman failed."; exit 2; }
# The corrupted variants: copy the full pilot pack, corrupt one model.
rm -rf "$PILOT_BAD" "$PILOT_BAD_NODE"
cp -R "$PILOT_DIR" "$PILOT_BAD"
cp -R "$PILOT_DIR" "$PILOT_BAD_NODE"
printf 'not a glb: A-018 corrupt-model fixture for the A-146 one-diagnostic grep' \
    > "$PILOT_BAD/models/units/rifleman.glb"
printf 'not a glb: A-018 corrupt-node fixture for the placeholder tier' \
    > "$PILOT_BAD_NODE/models/nodes/ore_node.glb"
log "A-018 packs: pilot at $PILOT_DIR; one-model at $PILOT_ONE; bad-model at $PILOT_BAD; bad-node at $PILOT_BAD_NODE"

# ---------------------------------------------------------------------------
# 4. Xvfb lifecycle.
# ---------------------------------------------------------------------------
pkill -f "Xvfb :$DISPLAY_NUM" >/dev/null 2>&1 || true
sleep 0.5
rm -f "/tmp/.X$DISPLAY_NUM-lock" "/tmp/.X11-unix/X$DISPLAY_NUM"
Xvfb ":$DISPLAY_NUM" -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB_PID=$!
cleanup() { kill "$XVFB_PID" >/dev/null 2>&1; }
trap cleanup EXIT INT TERM
for _ in $(seq 1 40); do
    [ -S "/tmp/.X11-unix/X$DISPLAY_NUM" ] && break
    kill -0 "$XVFB_PID" >/dev/null 2>&1 || break
    sleep 0.25
done
if ! kill -0 "$XVFB_PID" >/dev/null 2>&1; then
    log "A-018 BLOCKER: Xvfb failed to start on $DISPLAY."
    exit 2
fi
log "A-018 env: Xvfb running on $DISPLAY (pid $XVFB_PID), screen 1280x800x24, no window manager"

# ---------------------------------------------------------------------------
# 5. The checks.
# ---------------------------------------------------------------------------
log ""
log "A-018 checks begin (evidence in $EVIDENCE_DIR)"
python3 - "$BIN" "$EVIDENCE_DIR" "$PILOT_DIR" "$PILOT_ONE" "$PILOT_BAD" "$PILOT_BAD_NODE" "$@" << 'PYEOF'
import os, re, subprocess, sys, time, zlib, struct
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN, EVID = sys.argv[1], sys.argv[2]
PILOT, ONE, BAD, BAD_NODE = sys.argv[3], sys.argv[4], sys.argv[5], sys.argv[6]
FILTERS = sys.argv[7:]

SMOKE = re.compile(r"windowed smoke: (\d+) frames presented")
MODEL_LINE = re.compile(r"pandemonium client — entity model '([a-z_]+)' loaded \((\d+) primitives, (\d+) triangles")
SKIN_LINE = re.compile(r"pandemonium client — entity skin '([a-z_]+)' loaded")
PACK_LINE = re.compile(r"pandemonium client — asset pack '([a-z_0-9]+)' \(schema (\d+)\) loaded")

def write_png(path, raw_bgra, w, h):
    """Writes one BGRA ZPixmap capture as a PNG (pure stdlib: zlib + struct).
    """
    rows = bytearray()
    stride = w * 4
    for y in range(h):
        base = y * stride
        row = bytearray(w * 3)
        for x in range(w):
            i = base + x * 4
            row[x*3] = raw_bgra[i+2]; row[x*3+1] = raw_bgra[i+1]; row[x*3+2] = raw_bgra[i]
        rows += b"\x00" + bytes(row)
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 6))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)

def run_client(pack, frames, log_name, settle=2.0):
    """Runs one --frames windowed session and returns (stdout, evidence)."""
    out_path = os.path.join(EVID, log_name)
    with open(out_path, "w") as log_file:
        proc = subprocess.Popen(
            [BIN, "--pack", pack, "--frames", str(frames)],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        time.sleep(settle)              # the window maps and presents
        out, _ = proc.communicate(timeout=900)
        log_file.write(out)
    return out

def check(name, description):
    if FILTERS and name not in FILTERS:
        return False
    print(f"A-018 {name}: {description}")
    return True

def report(name, errs):
    if errs:
        print(f"A-018 {name}: FAIL")
        for err in errs:
            print(f"  - {err}")
        return False
    print(f"A-018 {name}: PASS")
    return True

results = []

# c0 — the pilot pack: schema 2, three model lines, clean (no diagnostics).
if check("c0", "the pilot pack loads all three D-002 models (schema 2), zero diagnostics, 900 frames"):
    out = run_client(PILOT, 900, "c0-pilot.log")
    errs = []
    m = PACK_LINE.search(out)
    if not m or m.group(1) != "pilot_d002" or m.group(2) != "2":
        errs.append(f"the pack line is not pilot_d002 schema 2: {m}")
    models = MODEL_LINE.findall(out)
    kinds = sorted(k for k, _, _ in models)
    if kinds != ["barracks", "ore_node", "rifleman"]:
        errs.append(f"the three model lines are wrong: {kinds}")
    tris = {k: int(t) for k, _, t in models}
    if tris.get("barracks", 0) == 0 or tris.get("ore_node", 0) == 0 or tris.get("rifleman", 0) == 0:
        errs.append(f"triangle counts missing: {tris}")
    skins = sorted(SKIN_LINE.findall(out))
    # The chain is lazy: model kinds never decode their skins — the six
    # skin-tier kinds only.
    if skins != ["command_center", "guardian", "raider", "supply_depot", "turret", "worker"]:
        errs.append(f"the skin lines are wrong (lazy chain expected): {skins}")
    if "pandemonium pack:" in out:
        errs.append("a clean pack printed a diagnostic")
    s = SMOKE.search(out)
    if not s or int(s.group(1)) != 900:
        errs.append(f"no 900-frame smoke line: {s}")
    results.append(report("c0", errs))

# c1 — the one-model pack: rifleman only.
if check("c1", "the one-model pack loads exactly one model (rifleman), the rest skin"):
    out = run_client(ONE, 300, "c1-one-model.log")
    errs = []
    models = MODEL_LINE.findall(out)
    if [k for k, _, _ in models] != ["rifleman"]:
        errs.append(f"expected exactly the rifleman model line, got {models}")
    skins = sorted(SKIN_LINE.findall(out))
    # rifleman renders its model, so its skin is not decoded; barracks and
    # ore_node fall to their skin/placeholder tiers.
    if "rifleman" in skins:
        errs.append("the model kind's skin was decoded (chain not lazy)")
    if "barracks" not in skins:
        errs.append("barracks fell through to its skin — expected the skin line")
    s = SMOKE.search(out)
    if not s or int(s.group(1)) != 300:
        errs.append(f"no 300-frame smoke line: {s}")
    results.append(report("c1", errs))

# c2 — the A-146 pin, live: exactly ONE diagnostic across a 900-frame run.
if check("c2", "a corrupted rifleman.glb + valid skin: exactly one diagnostic in 900 frames, skin renders"):
    out = run_client(BAD, 900, "c2-bad-model.log")
    errs = []
    count = out.count("entity 'rifleman' —")
    if count != 1:
        errs.append(f"the rifleman diagnostic appears {count} times, expected exactly 1")
    if "rendering the PNG skin tier" not in out:
        errs.append("the one line does not name the PNG skin tier")
    if "model 'models/units/rifleman.glb' could not be loaded" not in out:
        errs.append("the one line does not carry the model failure")
    if not SKIN_LINE.search(out) or "rifleman" not in SKIN_LINE.findall(out):
        errs.append("the rifleman skin did not load (the fallback tier must render)")
    # Every other kind's lines are untouched (per-entry independence).
    models = sorted(k for k, _, _ in MODEL_LINE.findall(out))
    if models != ["barracks", "ore_node"]:
        errs.append(f"the other models did not load: {models}")
    # Zero per-frame logging: the diagnostic printed once across 900 frames
    # (count == 1 above IS that proof), and no other pack diagnostics.
    other = [line for line in out.splitlines() if line.startswith("pandemonium pack:")
             and "rifleman" not in line]
    if other:
        errs.append(f"unexpected extra diagnostics: {other}")
    s = SMOKE.search(out)
    if not s or int(s.group(1)) != 900:
        errs.append(f"no 900-frame smoke line: {s}")
    results.append(report("c2", errs))

# c3 — the placeholder tier: a corrupted ore_node.glb with no skin.
if check("c3", "a corrupted ore_node.glb (no skin declared): one diagnostic, the placeholder tier"):
    out = run_client(BAD_NODE, 300, "c3-bad-node.log")
    errs = []
    count = out.count("entity 'ore_node' —")
    if count != 1:
        errs.append(f"the ore_node diagnostic appears {count} times, expected exactly 1")
    if "rendering the placeholder silhouette tier" not in out:
        errs.append("the one line does not name the placeholder tier")
    models = sorted(k for k, _, _ in MODEL_LINE.findall(out))
    if models != ["barracks", "rifleman"]:
        errs.append(f"the other models did not load: {models}")
    s = SMOKE.search(out)
    if not s or int(s.group(1)) != 300:
        errs.append(f"no 300-frame smoke line: {s}")
    results.append(report("c3", errs))

# c4 — the frame timeline: an AI-vs-AI spectate match on the pilot pack at
# the default camera; dated frames captured to the evidence dir. The three
# report frames (rifleman group, barracks, ore node) are selected from this
# timeline; the machine proves they are live renders, not void.
if check("c4", "an AI-vs-AI spectate match on the pilot pack captures a frame timeline"):
    d = display.Display(os.environ.get("DISPLAY"))
    root = d.screen().root
    # Keysyms -> keycodes (the a013 recipe: XTEST fakes keycodes, and the
    # keysym-to-keycode map is the server's own).
    SYMS = {"Return": 0xFF0D, "Right": 0xFF53, "Down": 0xFF54}
    keycodes = {}
    for name, sym in SYMS.items():
        kc = d.keysym_to_keycode(sym)
        if kc == 0:
            print(f"A-018 c4: FAIL")
            print(f"  - no keycode for {name} in the Xvfb keymap")
            results.append(False)
            keycodes = None
            break
        keycodes[name] = kc
    if keycodes is not None:
        def key(name):
            xtest.fake_input(d, X.KeyPress, detail=keycodes[name])
            d.flush(); time.sleep(0.4)
            xtest.fake_input(d, X.KeyRelease, detail=keycodes[name])
            d.flush(); time.sleep(0.4)
        def focus():
            pass  # focus goes to the client's own window once found below
        def close(window):
            protocols = d.intern_atom("WM_PROTOCOLS", False)
            delete = d.intern_atom("WM_DELETE_WINDOW", False)
            ev = xevent.ClientMessage(window=window, client_type=protocols,
                data=(32, [int(delete), X.CurrentTime, 0, 0, 0]))
            window.send_event(ev)
            d.sync()

        log_path = os.path.join(EVID, "c4-spectate.log")
        with open(log_path, "w") as log_file, \
             subprocess.Popen([BIN, "--pack", PILOT],
                              stdout=log_file, stderr=subprocess.STDOUT) as proc:
            # Find the client's own window (the a013 recipe: the new child
            # that appeared after launch) — focus goes THERE, not the root;
            # without a window manager the root focus would eat the keys.
            before = {w.id for w in root.query_tree().children}
            win = None
            t0 = time.time()
            while time.time() - t0 < 30:
                children = root.query_tree().children
                new = [w for w in children if w.id not in before]
                if new:
                    win = new[-1]
                    break
                if proc.poll() is not None:
                    break
                time.sleep(0.2)
            if win is None:
                errs = ["the client window never appeared"]
                proc.kill()
                results.append(report("c4", errs))
                frames = []
            else:
                win.set_input_focus(X.RevertToParent, X.CurrentTime)
                d.flush()
                time.sleep(6.0)        # the window maps at the main menu
                # New Match (row 0) -> mode Right (PlayerVsAi -> AiVsAi
                # spectate) -> Down x3 (Start) -> Enter. The default camera
                # watches the start; the AI plays both sides from there.
                key("Return")
                key("Right")
                key("Down"); key("Down"); key("Down")
                key("Return")
                # Capture a timeline: every 15 s for 4 minutes.
                W, H = 1280, 800
                frames = []
                for step in range(16):
                    time.sleep(15.0)
                    raw = root.get_image(0, 0, W, H, X.ZPixmap, 0xffffffff).data
                    stamp = int(time.time())
                    name = f"frame-{step:02d}-{stamp}.png"
                    write_png(os.path.join(EVID, name), raw, W, H)
                    frames.append((step, stamp, name, raw))
                    if proc.poll() is not None:
                        break
                close(win)               # graceful WM_DELETE to the window
            try:
                proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                proc.kill()
        errs = []
        if not frames:
            errs.append("no frames captured")
        for step, _, name, raw in frames:
            # A live render has variance; a void screen is a flat buffer.
            sample = raw[0::997]
            if len(set(sample)) < 8:
                errs.append(f"{name} looks void (no pixel variance)")
                break
        with open(log_path) as f:
            match_log = f.read()
        if "entity model 'rifleman'" not in match_log:
            errs.append("the spectate run did not load the rifleman model")
        results.append(report("c4", errs))
        print(f"A-018 c4: {len(frames)} frames in {EVID}/frame-*.png (the report's three are selected from these)")

print()
if all(results) and results:
    print(f"A-018: {sum(results)}/{len(results)} checks PASS")
    sys.exit(0)
print(f"A-018: {sum(results)}/{len(results)} checks PASS — FAILURES ABOVE")
sys.exit(1)
PYEOF
STATUS=$?
log ""
if [ "$STATUS" -eq 0 ]; then
    log "A-018 machine pass: all checks green (evidence in $EVIDENCE_DIR)"
else
    log "A-018 machine pass: FAILURES (evidence in $EVIDENCE_DIR)"
fi
exit $STATUS
