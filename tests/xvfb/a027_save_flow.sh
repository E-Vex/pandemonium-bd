#!/usr/bin/env bash
# A-027 — Xvfb + XTEST machine verification of B-027's mid-mission save
# flow (DEBT-205's repayment: the a012 shape, applied to the save).
#
# Verifies, as rendered surfaces driven by real XTEST input (not as unit
# logic), the save surfaces B-027 added:
#
#   c1     the pause menu's Save & Quit rung (row 7, mission-only) over a
#          live mission: Down x7 lands on it, Enter opens the confirm
#          screen over the live match (tick floor)
#   c2     the confirm gate's default focus is Cancel (A-009's law): Enter
#          returns to the pause menu with the match still live
#   c3     the confirmed Save & Quit writes the save and lands at the main
#          menu — the host drops on the way out, the save line names the
#          mission, the tick, and the file path, and the record IS on
#          disk at that path (asserted as a file artifact)
#   c4     the main menu's Continue row (row 5, appended after Quit —
#          A-219) resumes the saved mission: the resumed line names the
#          same mission at the SAME tick, the match re-enters unpaused
#          with no intro cutscene, and the exit tick has climbed past the
#          saved point by a live-match floor (a restart would be a few
#          seconds old; a cutscene would hold the tick at the saved
#          point) — the ADR's "re-hosts from the saved tick", on screen
#   c5     the save persists across a resume (A-215: the Continue row
#          keeps offering the saved point): a THIRD session over the same
#          config still carries the Continue row and resumes the same
#          saved point again
#   c6     a save-less main menu has no Continue row (A-219's law — the
#          appended row exists exactly when a save resolved): Down x5 on
#          the five-row menu wraps to row 0 (New Match), Enter opens the
#          new match screen — if a Continue row existed, Down x5 would
#          land on it and Enter would resume (a different screen)
#   c0     environment gate: the windowed client presents frames under
#          Xvfb + llvmpipe at the main menu (--frames 60, the summary
#          line), with the audio arm read off the same log
#
# The recipe is AI-Handoff.md §9 (DEBT-008), the a012 pattern script
# (tests/xvfb/a012_mission_screens.sh) verbatim where it applies: Xvfb run
# directly on :99 (no xvfb-run, no window manager), keyboard focus via
# python-xlib set_input_focus, input injected via XTEST (python-xlib
# xtest_fake_input — the §9 fallback when libXtst/xdotool are missing),
# graceful close via the WM_DELETE_WINDOW ClientMessage, wgpu's GL backend
# on llvmpipe (WGPU_BACKEND=gl, LIBGL_ALWAYS_SOFTWARE=1, a userland EGL
# stack when the system one is missing), key cadence >= 0.25-0.4 s.
#
# Per-user state isolation (the a026 addition to the a012 shape): every
# session runs with XDG_CONFIG_HOME pointed at a scratch directory under
# the evidence dir, so the save and the progress the sessions write never
# touch the operator's real config — the c3 save IS the c4/c5 Continue
# row's save (one scratch config, carried across those checks' sessions
# by design), and c1/c2/c6 run on a wiped one.
#
# Each check is one fresh client session per evidence line: launch under
# Xvfb, drive the keys, send WM_DELETE_WINDOW, capture the exit evidence
# line ("pandemonium client - exiting at the ...") from stdout, and
# compare it against the expected screen / tick / match count.
#
# Re-runnable: `bash tests/xvfb/a027_save_flow.sh [check ...]`
# (no arguments = all checks; the names are c0..c6 — run c3 before c4/c5:
# the Continue row needs the save c3 writes). Environment knobs:
#   A027_EVIDENCE_DIR  where the per-check logs go
#                      (default: <repo>/target/a027-evidence)
#   A027_USERLAND      the userland libEGL/libXtst tree (default:
#                      $HOME/.local/share/pandemonium/userland-libs; it is
#                      bootstrapped via `apt-get download` + `dpkg -x` if
#                      missing — no root needed)
#   A027_DISPLAY       the Xvfb display number (default 99)
#   A027_SEED          the pre-filled match seed (default 41008)
#
# Never runs the client outside Xvfb. Never modifies source files.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${A027_EVIDENCE_DIR:-$REPO_ROOT/target/a027-evidence}"
USERLAND="${A027_USERLAND:-$HOME/.local/share/pandemonium/userland-libs}"
DISPLAY_NUM="${A027_DISPLAY:-99}"
SEED="${A027_SEED:-41008}"
mkdir -p "$EVIDENCE_DIR"

log()  { printf '%s\n' "$*"; }

# ---------------------------------------------------------------------------
# 1. Hard environment requirements (AI-Handoff.md §9). A missing piece is a
#    reported blocker, never a substitution.
# ---------------------------------------------------------------------------
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
for tool in Xvfb python3 cargo; do
    command -v "$tool" >/dev/null 2>&1 || {
        log "A-027 BLOCKER: '$tool' is not installed and cannot be substituted."
        exit 2
    }
done
if ! python3 -c "import Xlib" >/dev/null 2>&1; then
    python3 -m pip install python-xlib >/dev/null 2>&1 \
        || python3 -m pip install --break-system-packages python-xlib >/dev/null 2>&1 \
        || { log "A-027 BLOCKER: python-xlib is not importable and pip cannot install it."; exit 2; }
fi

# ---------------------------------------------------------------------------
# 2. The userland library tree (no root: apt-get download + dpkg -x, per
#    AI-Handoff.md §9 — the same tree the A-010/A-012/A-026 scripts build).
#    Skipped when the system already has everything or the tree exists.
# ---------------------------------------------------------------------------
needs_userland() {
    ldconfig -p 2>/dev/null | grep -q "libEGL.so.1" || return 0
    ldconfig -p 2>/dev/null | grep -q "libXtst.so.6" || return 0
    ldconfig -p 2>/dev/null | grep -q "libxkbcommon-x11.so.0" || return 0
    return 1
}
if [ -d "$USERLAND/usr/lib/x86_64-linux-gnu" ]; then
    log "A-027 env: userland library tree at $USERLAND (pre-existing)"
elif needs_userland; then
    log "A-027 env: bootstrapping the userland library tree at $USERLAND ..."
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
    ) || { log "A-027 BLOCKER: the userland library bootstrap failed (apt-get download / dpkg -x)."; exit 2; }
    rm -rf "$DEB_DIR"
    LIBD="$USERLAND/usr/lib/x86_64-linux-gnu"
    [ -e "$LIBD/libxkbcommon.so" ]     || ln -s libxkbcommon.so.0 "$LIBD/libxkbcommon.so"
    [ -e "$LIBD/libxkbcommon-x11.so" ] || ln -s libxkbcommon-x11.so.0 "$LIBD/libxkbcommon-x11.so"
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
    log "A-027 env: userland library tree bootstrapped at $USERLAND"
else
    log "A-027 env: system EGL/XTEST libraries present; userland tree not needed"
    USERLAND=""
fi

if [ -n "$USERLAND" ]; then
    export LD_LIBRARY_PATH="$USERLAND/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
    export LIBRARY_PATH="$USERLAND/usr/lib/x86_64-linux-gnu${LIBRARY_PATH:+:$LIBRARY_PATH}"
    [ -f "$USERLAND/usr/lib/x86_64-linux-gnu/pkgconfig/alsa.pc" ] \
        && export PKG_CONFIG_PATH="$USERLAND/usr/lib/x86_64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    export __EGL_VENDOR_LIBRARY_DIRS="$USERLAND/glvnd/egl_vendor.d"
fi

# The §9 renderer environment: wgpu's GL backend on Mesa llvmpipe.
export WGPU_BACKEND=gl
export LIBGL_ALWAYS_SOFTWARE=1
export DISPLAY=":$DISPLAY_NUM"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/xdg-runtime}"
mkdir -p "$XDG_RUNTIME_DIR" 2>/dev/null || true

# The scratch per-user config (the a026 shape): the save and the progress
# the sessions write land here — never the operator's real config
# directory. Created fresh when missing; the per-check wipes (below) keep
# the state honest — c1/c2/c3/c6 wipe it, c4/c5 deliberately carry c3's
# save (a filtered re-run of c4/c5 needs a c3 run first: the header's
# "run c3 before c4/c5").
export XDG_CONFIG_HOME="$EVIDENCE_DIR/user-config"
mkdir -p "$XDG_CONFIG_HOME"

# ---------------------------------------------------------------------------
# 3. Environment evidence (the report's §1 input).
# ---------------------------------------------------------------------------
{
    echo "A-027 environment ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
    echo "Xvfb: $(command -v Xvfb)$(dpkg -l xvfb 2>/dev/null | awk '/^ii/ {print " ("$3")"}')"
    echo "python: $(python3 --version 2>&1), python-xlib: $(python3 -c 'import Xlib; print(Xlib.__version__)' 2>&1)"
    echo "XTEST injection: python-xlib xtest_fake_input (libXtst/xdotool not required)"
    echo "renderer: WGPU_BACKEND=$WGPU_BACKEND LIBGL_ALWAYS_SOFTWARE=$LIBGL_ALWAYS_SOFTWARE (Mesa llvmpipe)"
    echo "llvmpipe driver: $(ls /usr/lib/x86_64-linux-gnu/dri/swrast_dri.so 2>/dev/null || echo 'MISSING')"
    [ -n "$USERLAND" ] && echo "userland tree: $USERLAND" || echo "userland tree: not needed (system EGL)"
    echo "seed: $SEED, display: $DISPLAY, scratch config: $XDG_CONFIG_HOME"
    echo
} | tee "$EVIDENCE_DIR/env.txt"

# ---------------------------------------------------------------------------
# 4. Xvfb lifecycle: run directly (xvfb-run is broken here), no WM (a WM
#    breaks the GL surface), screen >= the 800x600 window.
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
    log "A-027 BLOCKER: Xvfb failed to start on $DISPLAY."
    exit 2
fi
log "A-027 env: Xvfb running on $DISPLAY (pid $XVFB_PID), screen 1280x800x24, no window manager"

# The XTEST extension probe (a missing extension is STOP 1 — never
# substituted).
if python3 - "$DISPLAY" << 'PYEOF'
import sys
from Xlib import display as xdisplay
d = xdisplay.Display(sys.argv[1])
ext = d.query_extension("XTEST")
if ext is None or not ext.present:
    print("A-027 BLOCKER: the Xvfb server on %s does not carry the XTEST "
          "extension — input injection is impossible." % sys.argv[1])
    sys.exit(2)
print("A-027 env: XTEST extension present on %s (major opcode %d)"
      % (sys.argv[1], ext.major_opcode))
PYEOF
then
    python3 - "$DISPLAY" << 'PYEOF' >> "$EVIDENCE_DIR/env.txt"
import sys
from Xlib import display as xdisplay
d = xdisplay.Display(sys.argv[1])
ext = d.query_extension("XTEST")
print("XTEST extension: present on %s (major opcode %d)" % (sys.argv[1], ext.major_opcode))
PYEOF
else
    exit 2
fi

# ---------------------------------------------------------------------------
# 5. Build the client.
# ---------------------------------------------------------------------------
log "A-027 build: cargo build -p pandemonium-client --release"
if ! (cd "$REPO_ROOT" && cargo build -p pandemonium-client --release 2>&1 | tail -3); then
    log "A-027 BLOCKER: the client failed to build."
    exit 2
fi
BIN="$REPO_ROOT/target/release/pandemonium-client"
[ -x "$BIN" ] || { log "A-027 BLOCKER: $BIN missing after the build."; exit 2; }

# ---------------------------------------------------------------------------
# 6. The check sessions (XTEST input, WM_DELETE_WINDOW close, evidence
#    line capture). The driver prints one verdict per check.
# ---------------------------------------------------------------------------
log ""
log "A-027 checks begin (evidence in $EVIDENCE_DIR)"
python3 - "$BIN" "$EVIDENCE_DIR" "$SEED" "$XDG_CONFIG_HOME" "$@" << 'PYEOF'
import os, re, json, shutil, subprocess, sys, time
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN, EVID, SEED, CFG = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
FILTERS = sys.argv[5:]

CADENCE = 0.35        # key cadence in seconds (>= 0.25-0.4 s per AI-Handoff §9)
READY_WAIT = 1.0      # settle after the window maps, before the first key
SCENE_WAIT = 1.5      # cutscene seconds before the skip (mid-pan, caption up)
LIVE_RUN = 8.0        # live-match seconds before the Esc (a real mission
                      # match under the pause-menu screens)
POST_RESUME = 6.0     # live-match seconds after a Continue (c4/c5: the
                      # resumed match must run past the saved point)
TPS = 30              # pandemonium_sim::TICKS_PER_SECOND
T_LIVE_MIN = 60       # >= 2 s of live mission match behind a screen
T_PAST_SAVE = 60      # c4/c5: >= 2 s of live match PAST the saved tick —
                      # a restart is a few seconds old (<= ~200 ticks under
                      # this harness) and a cutscene hold never moves; with
                      # the saved tick ~240, saved+60 is unreachable for
                      # both wrong readings

EXIT_HOST = re.compile(r"exiting at the ([a-z ]+?) \(tick (\d+), (\d+) commands submitted\)")
EXIT_MENU = re.compile(r"exiting at the ([a-z ]+?) \((\d+) match(?:es)? started this run\)")
SMOKE = re.compile(r"windowed smoke: (\d+) frames presented, at the ([a-z ]+?) \(([^)]*)\)")
AUDIO = re.compile(r"— audio: (.+)")
SAVE_LINE = re.compile(r"mission save: '([^']+)' at tick (\d+) \(hash (0x[0-9a-f]+)\) saved: (.+?)\s*$")
RESUMED_LINE = re.compile(r"mission save resumed: '([^']+)' at tick (\d+) \(hash (0x[0-9a-f]+)\)")

def K(name):  return ("k", name)
def S(sec):   return ("s", sec)
def SHOT(f):  return ("shot", f)
WIPE = "wipe"                   # plan marker: reset the scratch config first
CLOSE = ("close",)

# main menu (campaign-present layout): row 0 New Match, row 1 Campaign,
# row 2 Settings, row 3 Controls, row 4 Quit, row 5 Continue (exactly when
# a save resolved — A-219's appended row).
MENU_TO_CAMPAIGN = [K("Down"), K("Enter")]                     # row 1 Campaign
CAMPAIGN_START   = [K("Down"), K("Down"), K("Enter")]          # row 2 Start
TO_CONTINUE      = [K("Down"), K("Down"), K("Down"), K("Down"), K("Down"), K("Enter")]  # row 5
# the mission path (campaign -> Start on chapter 1 -> cutscene -> live):
MISSION_START = MENU_TO_CAMPAIGN + CAMPAIGN_START
SKIP_CUTSCENE = [S(SCENE_WAIT), K("Space")]
PAUSE_PREFIX  = MISSION_START + SKIP_CUTSCENE + [S(LIVE_RUN), K("Escape")]
TO_PAUSE_SAVE = [K("Down"), K("Down"), K("Down"), K("Down"), K("Down"), K("Down"),
                 K("Down"), K("Enter")]                        # row 7 Save & Quit
CONFIRM       = [K("Down"), K("Enter")]                        # confirm row 1

CHECKS = [
    ("c0", "environment gate: the windowed client presents 60 frames at the main menu",
     [WIPE, ("c0", [], ["--frames", "60"],
       {"smoke": True, "frames": 60, "screen": "main menu", "session": "no match this run"})]),
    ("c1", "the Save & Quit rung (row 7) exists over a live mission: Enter opens the confirm screen over the live match",
     [WIPE, ("c1", PAUSE_PREFIX + TO_PAUSE_SAVE + [CLOSE], [],
       {"screen": "confirm screen", "tick_min": T_LIVE_MIN})]),
    ("c2", "the confirm gate's default focus is Cancel: Enter keeps the match (the pause menu, still live)",
     [WIPE, ("c2", PAUSE_PREFIX + TO_PAUSE_SAVE + [K("Enter"), CLOSE], [],
       {"screen": "pause menu", "tick_min": T_LIVE_MIN})]),
    ("c3", "the confirmed Save & Quit writes the save (the line names mission, tick, path; the record IS on disk) and lands at the main menu",
     [WIPE, ("c3", PAUSE_PREFIX + TO_PAUSE_SAVE + CONFIRM + [S(3.0), CLOSE], [],
       {"screen": "main menu", "matches": 1, "save_written": True})]),
    ("c4", "the Continue row (row 5) resumes the saved mission: the resumed line names the same mission at the saved tick, the match re-enters unpaused with no cutscene, and the exit tick climbs past the saved point",
     [("c4", TO_CONTINUE + [S(POST_RESUME), SHOT("c4-resumed.png"), CLOSE], [],
       {"screen": "match", "tick_min": ("saved", T_PAST_SAVE),
        "resumed_line": True})]),
    ("c5", "the save persists across a resume (A-215): a third session still carries the Continue row and resumes the same saved point again",
     [("c5", TO_CONTINUE + [S(POST_RESUME), CLOSE], [],
       {"screen": "match", "tick_min": ("saved", T_PAST_SAVE),
        "resumed_line": True})]),
    ("c6", "a save-less main menu has no Continue row (A-219): Down x5 wraps to New Match, Enter opens the new match screen",
     [WIPE, ("c6", [K("Down"), K("Down"), K("Down"), K("Down"), K("Down"), K("Enter"), CLOSE], [],
       {"screen": "new match screen", "matches": 0})]),
]

STATE = {}  # cross-check state: what c3's save line recorded (persisted to
# EVID/state.json so a filtered c4/c5 re-run finds c3's save — the full
# pass runs in one process, a chunked one does not)
STATE_PATH = os.path.join(EVID, "state.json")
try:
    with open(STATE_PATH, "r") as f:
        STATE = json.load(f)
except (OSError, ValueError):
    STATE = {}

class Driver:
    def __init__(self):
        self.d = display.Display(os.environ.get("DISPLAY"))
        self.root = self.d.screen().root
        self.keycodes = {}
        for name, sym in (("Up", 0xFF52), ("Down", 0xFF54),
                          ("Left", 0xFF51), ("Right", 0xFF53),
                          ("Enter", 0xFF0D), ("Escape", 0xFF1B),
                          ("Space", 0x0020)):
            kc = self.d.keysym_to_keycode(sym)
            if kc == 0:
                raise SystemExit(f"A-027 BLOCKER: no keycode for {name} in the Xvfb keymap")
            self.keycodes[name] = kc
        self.wm_protocols = self.d.intern_atom("WM_PROTOCOLS")
        self.wm_delete = self.d.intern_atom("WM_DELETE_WINDOW")

    def key(self, name):
        kc = self.keycodes[name]
        xtest.fake_input(self.d, X.KeyPress, detail=kc)
        self.d.flush()
        time.sleep(CADENCE)
        xtest.fake_input(self.d, X.KeyRelease, detail=kc)
        self.d.flush()
        time.sleep(CADENCE)

    def send_close(self, win):
        ev = xevent.ClientMessage(window=win, client_type=self.wm_protocols,
                                  data=(32, [self.wm_delete, 0, 0, 0, 0]))
        self.d.send_event(win, ev, event_mask=X.NoEventMask)
        self.d.flush()

    def screenshot(self, win, name):
        """Capture the window to EVID/<name> (supplementary evidence — a
        failure is reported, never a verdict)."""
        path = os.path.join(EVID, name)
        try:
            import numpy
            from PIL import Image
            geom = win.get_geometry()
            w = min(geom.width, 1280)
            h = min(geom.height, 800)
            img = win.get_image(0, 0, w, h, X.ZPixmap, 0xFFFFFFFF)
            arr = numpy.frombuffer(img.data, dtype=numpy.uint8)[: w * h * 4]
            arr = arr.reshape(h, w, 4)[:, :, 2::-1]  # ZPixmap BGRX -> RGB
            Image.fromarray(arr).save(path)
            return path
        except Exception as exc:  # noqa: BLE001 — soft evidence, never fatal
            print(f"  screenshot {name}: unavailable ({exc})")
            return None

    def run(self, name, steps, extra_args):
        """One session: launch, drive, close, return the parsed evidence."""
        log_path = os.path.join(EVID, name + ".log")
        shots = []
        before = {w.id for w in self.root.query_tree().children}
        with open(log_path, "w") as log:
            t0 = time.time()
            proc = subprocess.Popen([BIN, "--seed", str(SEED)] + list(extra_args),
                                    stdout=log, stderr=subprocess.STDOUT,
                                    cwd=os.path.dirname(BIN))
            win = None
            while time.time() - t0 < 30:
                children = self.root.query_tree().children
                new = [w for w in children if w.id not in before]
                if new:
                    win = new[-1]
                    break
                if proc.poll() is not None:
                    break
                time.sleep(0.2)
            if win is not None:
                win.set_input_focus(X.RevertToParent, X.CurrentTime)
                self.d.flush()
                time.sleep(READY_WAIT)
            for step in steps:
                if proc.poll() is not None:
                    break
                if step[0] == "k" and win is not None:
                    self.key(step[1])
                elif step[0] == "s":
                    time.sleep(step[1])
                elif step[0] == "shot" and win is not None:
                    path = self.screenshot(win, step[1])
                    if path:
                        shots.append(path)
                elif step[0] == "close" and win is not None:
                    self.send_close(win)
            try:
                proc.wait(timeout=60)
                rc = proc.returncode
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
                rc = None
        with open(log_path, "r", errors="replace") as f:
            lines = f.read().splitlines()
        return {"rc": rc, "lines": lines, "shots": shots}

def parse_evidence(result):
    smoke = host = menu = None
    audio = saved = resumed = None
    for line in result["lines"]:
        m = SMOKE.search(line)
        if m:
            smoke = {"kind": "smoke", "frames": int(m.group(1)),
                     "screen": m.group(2), "session": m.group(3)}
        m = EXIT_HOST.search(line)
        if m:
            host = {"kind": "host", "screen": m.group(1),
                    "tick": int(m.group(2)), "commands": int(m.group(3))}
        m = EXIT_MENU.search(line)
        if m:
            menu = {"kind": "menu", "screen": m.group(1),
                    "matches": int(m.group(2))}
        m = AUDIO.search(line)
        if m:
            audio = m.group(1)
        m = SAVE_LINE.search(line)
        if m:
            saved = {"mission": m.group(1), "tick": int(m.group(2)),
                     "hash": m.group(3), "path": m.group(4)}
        m = RESUMED_LINE.search(line)
        if m:
            resumed = {"mission": m.group(1), "tick": int(m.group(2)),
                       "hash": m.group(3)}
    return smoke, host, menu, audio, saved, resumed

def scan_pixels(path, region):
    """Soft rendered-surface evidence: count the HUD color classes inside
    a region (warm-yellow header, objective green, blue-white text).
    Returns None when the readback is unusable."""
    try:
        import numpy
        from PIL import Image
        im = Image.open(path).convert("RGB")
        a = numpy.asarray(im)
        x0, y0, x1, y1 = region
        a = a[y0:y1, x0:x1].astype(int)
        warm = int(((a[:, :, 0] > 190) & (a[:, :, 1] > 165)
                    & (a[:, :, 2] < 180) & (a[:, :, 0] > a[:, :, 2] + 50)).sum())
        green = int(((a[:, :, 1] > 190) & (a[:, :, 1] > a[:, :, 0] + 50)
                     & (a[:, :, 1] > a[:, :, 2] + 50)).sum())
        white = int(((a[:, :, 0] > 200) & (a[:, :, 1] > 205)
                     & (a[:, :, 2] > 215) & (a[:, :, 2] > a[:, :, 0] + 8)).sum())
        return {"warm-yellow": warm, "objective-green": green, "blue-white": white}
    except Exception:  # noqa: BLE001 — soft evidence, never fatal
        return None

def session_verdict(result, expect):
    errs = []
    smoke, host, menu, audio, saved, resumed = parse_evidence(result)
    evidence = smoke if expect.get("smoke") else (host or menu)
    if result["rc"] not in (0,):
        errs.append(f"client exit code {result['rc']} (expected a clean 0)")
    if evidence is None:
        if expect.get("smoke"):
            errs.append("no --frames windowed-smoke summary line")
        elif smoke is not None:
            errs.append("the session ended at the --frames budget (no driven exit)")
        else:
            errs.append("no exit evidence line in the session log")
    else:
        if expect.get("smoke"):
            if expect.get("frames") is not None and evidence.get("frames") != expect["frames"]:
                errs.append(f"frames {evidence.get('frames')} != {expect['frames']}")
            if expect.get("session") and expect["session"] not in evidence.get("session", ""):
                errs.append(f"session '{evidence.get('session')}' lacks '{expect['session']}'")
        if "screen" in expect and evidence.get("screen") != expect["screen"]:
            errs.append(f"screen '{evidence.get('screen')}' != '{expect['screen']}'")
        if "matches" in expect and evidence.get("matches") != expect["matches"]:
            errs.append(f"matches started {evidence.get('matches')} != {expect['matches']}")
        if "tick_min" in expect:
            floor = expect["tick_min"]
            if isinstance(floor, tuple) and floor[0] == "saved":
                base = STATE.get("saved_tick")
                if base is None:
                    errs.append("no saved tick recorded (run c3 first)")
                    base = 0
                floor = base + floor[1]
            if (evidence.get("tick") or 0) < floor:
                errs.append(f"tick {evidence.get('tick')} < {floor}")
    # The save-write evidence (c3): the line names the mission, the tick,
    # and the path; the record exists on disk at that path.
    if expect.get("save_written"):
        if saved is None:
            errs.append("no mission-save line in the session log")
        else:
            STATE["saved_tick"] = saved["tick"]
            STATE["saved_mission"] = saved["mission"]
            with open(STATE_PATH, "w") as f:
                json.dump(STATE, f)
            print(f"  [c3] save line: '{saved['mission']}' at tick {saved['tick']} "
                  f"({saved['hash']}) -> {saved['path']}")
            if not os.path.isfile(saved["path"]):
                errs.append(f"the save file is not on disk at {saved['path']}")
    # The resume evidence (c4/c5): the resumed line names the same mission
    # at the SAME saved tick (the rehost enters exactly there).
    if expect.get("resumed_line"):
        if resumed is None:
            errs.append("no mission-save-resumed line in the session log")
        else:
            print(f"  [{sname}] resumed line: '{resumed['mission']}' at tick {resumed['tick']}")
            if STATE.get("saved_mission") is not None:
                if resumed["mission"] != STATE["saved_mission"]:
                    errs.append(f"resumed mission '{resumed['mission']}' != saved '{STATE['saved_mission']}'")
            if STATE.get("saved_tick") is not None and resumed["tick"] != STATE["saved_tick"]:
                errs.append(f"resumed tick {resumed['tick']} != saved tick {STATE['saved_tick']}")
    return errs, evidence, audio

def fmt_drive(steps):
    return " ".join(step[1] if step[0] == "k" else
                    (f"[{step[1]}s]" if step[0] == "s" else
                     (f"[shot {step[1]}]" if step[0] == "shot" else "[close]"))
                    for step in steps)

drv = Driver()
passed = failed = 0
for name, desc, plan in CHECKS:
    if FILTERS and name not in FILTERS:
        continue
    print(f"[A-027 {name}] {desc}")
    check_ok = True
    if WIPE in plan:
        shutil.rmtree(CFG, ignore_errors=True)
        os.makedirs(CFG, exist_ok=True)
        # A wiped config invalidates any recorded save state (c3 re-records
        # its own; c4/c5 never wipe).
        if name != "c3":
            try:
                os.unlink(STATE_PATH)
            except OSError:
                pass
            STATE.clear()
        print(f"  [{name}] scratch config wiped (fresh state)")
    sessions = [s for s in plan if s != WIPE]
    for sname, steps, extra_args, expect in sessions:
        result = drv.run(sname, steps, extra_args)
        errs, evidence, audio = session_verdict(result, expect)
        line_ev = next((l for l in reversed(result["lines"])
                        if "exiting at the" in l or "windowed smoke" in l), "(none)")
        print(f"  [{sname}] drive: {fmt_drive(steps)}")
        print(f"  [{sname}] evidence: {line_ev}")
        if audio and sname == "c0":
            print(f"  [{sname}] audio arm: {audio}")
        for shot in result["shots"]:
            note = ""
            if "c4-resumed" in shot:
                counts = scan_pixels(shot, (0, 30, 420, 150))
                if counts:
                    note = (" — HUD scan %s (MISSION header warm-yellow, the "
                            "objective lines blue-white — the resumed match's "
                            "seeded tracker, top-left block)" % counts)
            print(f"  [{sname}] screenshot: {os.path.basename(shot)}{note}")
        if errs:
            print(f"  [{sname}] verdict: FAIL ({'; '.join(errs)})")
            check_ok = False
        else:
            print(f"  [{sname}] verdict: PASS")
    if check_ok:
        passed += 1
        print(f"[A-027 {name}] verdict: PASS")
    else:
        failed += 1
        print(f"[A-027 {name}] verdict: FAIL")
    print()
    if name == "c0" and not check_ok:
        # The environment gate: every later check needs the windowed client
        # to present frames under Xvfb — STOP here rather than pile up
        # meaningless failures (the brief's STOP 3 shape).
        print("A-027 BLOCKER: the c0 environment gate failed — the client "
              "did not run cleanly under Xvfb (see c0.log). No further "
              "checks were attempted.")
        sys.exit(2)

print(f"=== A-027 Xvfb+XTEST pass: {passed} passed, {failed} failed"
      f" ({passed + failed} checks run) ===")
sys.exit(1 if failed else 0)
PYEOF
DRIVER_RC=$?

log ""
if [ "$DRIVER_RC" -eq 0 ]; then
    log "A-027 RESULT: all checks green. Evidence: $EVIDENCE_DIR"
else
    log "A-027 RESULT: FAILURES PRESENT (driver rc=$DRIVER_RC). Evidence: $EVIDENCE_DIR"
fi
exit "$DRIVER_RC"
