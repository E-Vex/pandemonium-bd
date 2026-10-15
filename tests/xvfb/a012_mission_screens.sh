#!/usr/bin/env bash
# A-012 — Xvfb + XTEST machine verification of A-011's mission screens.
#
# Verifies, as rendered surfaces driven by real XTEST input (not as unit
# logic), the four mission-screen surfaces A-011 added:
#
#   c1     main menu -> New Match -> Missions row (row 5) -> the mission
#          picker screen
#   c2     the picker's Mission row cycles the tree's mission list
#          (Left/Right — the map field's wrap law; on the one-mission
#          Phase 1 tree the index wraps to itself, no crash, the picker
#          stays), and Start on the picker opens the mission-start
#          cutscene over a paused host (the exit line reads "match" with
#          the tick frozen at 0 two seconds in — an unpaused match would
#          already be ~60 ticks)
#   c3     the cutscene plays (caption panel screenshot, supplementary)
#          and any key skips it: the host unpauses and the match begins
#          (the exit tick has climbed past a fresh-match floor). A failed
#          skip leaves the 6 s scene running past this session's close:
#          tick 0 — the two readings are disjoint
#   c4     the objective HUD rides the live mission match: the match runs
#          ticks after the skip (exit "match", tick floor) and a
#          screenshot of the HUD block is captured (the MISSION header /
#          [x]-green / [ ]-white lines are scanned as supplementary
#          rendered evidence, not a verdict)
#   c5     Esc during a mission match opens the pause menu over the live
#          mission match (tick floor proves a real match underneath)
#   c6     pause -> Controls (row 2) is entered over the paused mission
#          match, and Back returns to the pause menu (two sessions)
#   c7     pause -> Resume: the SAME mission continues (the tick floor is
#          unreachable for a restart — drop + reconstruct would land on a
#          fresh host a few seconds old)
#   c8     pause -> Quit to Menu (row 4) opens the Confirm screen, and
#          Enter on the default focus (Cancel) returns to the pause menu
#          with the match still live (two sessions)
#   c0     environment gate: the windowed client presents frames under
#          Xvfb + llvmpipe at the main menu (--frames 60, the summary
#          line), with the audio arm read off the same log
#
# The recipe is AI-Handoff.md §9 (DEBT-008), the A-010 pattern script
# (tests/xvfb/a010_controls_confirm.sh) verbatim where it applies: Xvfb
# run directly on :99 (no xvfb-run, no window manager), keyboard focus
# via python-xlib set_input_focus, input injected via XTEST
# (python-xlib xtest_fake_input — the §9 fallback when libXtst/xdotool
# are missing), graceful close via the WM_DELETE_WINDOW ClientMessage,
# wgpu's GL backend on llvmpipe (WGPU_BACKEND=gl,
# LIBGL_ALWAYS_SOFTWARE=1, a userland EGL stack when the system one is
# missing), key cadence >= 0.25-0.4 s.
#
# Each check is one fresh client session per evidence line (c2/c6/c8
# carry two: the exit line prints only once per session, so screens that
# need a second reading — Back, Cancel — get their own launch): launch
# under Xvfb, drive the keys, send WM_DELETE_WINDOW, capture the exit
# evidence line ("pandemonium client - exiting at the ...") from stdout,
# and compare it against the expected screen / tick / match count.
#
# Re-runnable: `bash tests/xvfb/a012_mission_screens.sh [check ...]`
# (no arguments = all checks; the names are c0..c8 — a compound check
# runs both its sessions). Environment knobs:
#   A012_EVIDENCE_DIR  where the per-check logs go
#                      (default: <repo>/target/a012-evidence)
#   A012_USERLAND      the userland libEGL/libXtst tree (default:
#                      $HOME/.local/share/pandemonium/userland-libs; it is
#                      bootstrapped via `apt-get download` + `dpkg -x` if
#                      missing — no root needed)
#   A012_DISPLAY       the Xvfb display number (default 99)
#   A012_SEED          the pre-filled match/mission seed (default 41008)
#
# Never runs the client outside Xvfb. Never modifies source files.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${A012_EVIDENCE_DIR:-$REPO_ROOT/target/a012-evidence}"
USERLAND="${A012_USERLAND:-$HOME/.local/share/pandemonium/userland-libs}"
DISPLAY_NUM="${A012_DISPLAY:-99}"
SEED="${A012_SEED:-41008}"
mkdir -p "$EVIDENCE_DIR"

log()  { printf '%s\n' "$*"; }

# ---------------------------------------------------------------------------
# 1. Hard environment requirements (AI-Handoff.md §9). A missing piece is a
#    reported blocker, never a substitution.
# ---------------------------------------------------------------------------
# rustup's userland cargo first, if present (no root needed).
if ! command -v cargo >/dev/null 2>&1 && [ -x "$HOME/.cargo/bin/cargo" ]; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
for tool in Xvfb python3 cargo; do
    command -v "$tool" >/dev/null 2>&1 || {
        log "A-012 BLOCKER: '$tool' is not installed and cannot be substituted."
        exit 2
    }
done
if ! python3 -c "import Xlib" >/dev/null 2>&1; then
    # python-xlib is pure Python and drives XTEST directly (no libXtst
    # needed) — the §9 fallback when xdotool/libXtst are unavailable.
    python3 -m pip install python-xlib >/dev/null 2>&1 \
        || python3 -m pip install --break-system-packages python-xlib >/dev/null 2>&1 \
        || { log "A-012 BLOCKER: python-xlib is not importable and pip cannot install it."; exit 2; }
fi

# ---------------------------------------------------------------------------
# 2. The userland library tree (no root: apt-get download + dpkg -x, per
#    AI-Handoff.md §9 / the A-009 session-start bootstrap — the same tree
#    the A-010 script builds): EGL (glvnd + Mesa), libXtst, xkbcommon-x11.
#    Skipped when the system already has everything or the tree exists.
# ---------------------------------------------------------------------------
needs_userland() {
    ldconfig -p 2>/dev/null | grep -q "libEGL.so.1" || return 0
    ldconfig -p 2>/dev/null | grep -q "libXtst.so.6" || return 0
    ldconfig -p 2>/dev/null | grep -q "libxkbcommon-x11.so.0" || return 0
    return 1
}
if [ -d "$USERLAND/usr/lib/x86_64-linux-gnu" ]; then
    log "A-012 env: userland library tree at $USERLAND (pre-existing)"
elif needs_userland; then
    log "A-012 env: bootstrapping the userland library tree at $USERLAND ..."
    DEB_DIR="$(mktemp -d)"
    (
        cd "$DEB_DIR" || exit 1
        # EGL + XTEST + XKB client libraries; libasound2-dev only when the
        # system pkg-config cannot find alsa (the client links rodio/cpal).
        PACKS="libegl1 libegl-mesa0 libgles2 libxtst6 libxkbcommon-x11-0 libxcb-xkb1"
        if ! pkg-config --exists alsa; then
            # the client links rodio/cpal -> alsa; the trixie runtime package
            # is libasound2t64 (the time-t rename), needed only if the
            # system does not already provide libasound.so.2.
            PACKS="$PACKS libasound2-dev"
            ldconfig -p 2>/dev/null | grep -q "libasound.so.2" || PACKS="$PACKS libasound2t64"
        fi
        ldconfig -p 2>/dev/null | grep -q "libgallium" || PACKS="$PACKS mesa-libgallium"
        # shellcheck disable=SC2086
        apt-get download $PACKS >/dev/null 2>&1 || { echo "apt-get download failed"; exit 1; }
        mkdir -p "$USERLAND"
        for deb in *.deb; do dpkg -x "$deb" "$USERLAND"; done
    ) || { log "A-012 BLOCKER: the userland library bootstrap failed (apt-get download / dpkg -x)."; exit 2; }
    rm -rf "$DEB_DIR"
    LIBD="$USERLAND/usr/lib/x86_64-linux-gnu"
    # Unversioned libxkbcommon symlinks — winit loads them by that name.
    [ -e "$LIBD/libxkbcommon.so" ]     || ln -s libxkbcommon.so.0 "$LIBD/libxkbcommon.so"
    [ -e "$LIBD/libxkbcommon-x11.so" ] || ln -s libxkbcommon-x11.so.0 "$LIBD/libxkbcommon-x11.so"
    # The glvnd vendor ICD json (the exact file shipped in libegl-mesa0).
    mkdir -p "$USERLAND/glvnd/egl_vendor.d"
    printf '%s\n' \
        '{' \
        '    "file_format_version" : "1.0.0",' \
        '    "ICD" : {' \
        '        "library_path" : "libEGL_mesa.so.0"' \
        '    }' \
        '}' > "$USERLAND/glvnd/egl_vendor.d/50_mesa.json"
    # alsa: make the dev tree self-contained (headers + resolvable .so +
    # a pkg-config file pointing at the userland paths).
    if [ -f "$LIBD/pkgconfig/alsa.pc" ]; then
        [ -e "$LIBD/libasound.so.2.0.0" ] || cp /usr/lib/x86_64-linux-gnu/libasound.so.2.0.0 "$LIBD/" 2>/dev/null || true
        sed -i "s|^prefix=.*|prefix=$USERLAND/usr|" "$LIBD/pkgconfig/alsa.pc"
    fi
    log "A-012 env: userland library tree bootstrapped at $USERLAND"
else
    log "A-012 env: system EGL/XTEST libraries present; userland tree not needed"
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

# ---------------------------------------------------------------------------
# 3. Environment evidence (the report's §1 input).
# ---------------------------------------------------------------------------
{
    echo "A-012 environment ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
    echo "Xvfb: $(command -v Xvfb)$(dpkg -l xvfb 2>/dev/null | awk '/^ii/ {print " ("$3")"}')"
    echo "python: $(python3 --version 2>&1), python-xlib: $(python3 -c 'import Xlib; print(Xlib.__version__)' 2>&1)"
    echo "XTEST injection: python-xlib xtest_fake_input (libXtst/xdotool not required)"
    echo "renderer: WGPU_BACKEND=$WGPU_BACKEND LIBGL_ALWAYS_SOFTWARE=$LIBGL_ALWAYS_SOFTWARE (Mesa llvmpipe)"
    echo "llvmpipe driver: $(ls /usr/lib/x86_64-linux-gnu/dri/swrast_dri.so 2>/dev/null || echo 'MISSING')"
    [ -n "$USERLAND" ] && echo "userland tree: $USERLAND" || echo "userland tree: not needed (system EGL)"
    echo "seed: $SEED, display: $DISPLAY"
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
    kill -0 "$XVFB_PID" 2>/dev/null || break
    sleep 0.25
done
if ! kill -0 "$XVFB_PID" 2>/dev/null; then
    log "A-012 BLOCKER: Xvfb failed to start on $DISPLAY."
    exit 2
fi
log "A-012 env: Xvfb running on $DISPLAY (pid $XVFB_PID), screen 1280x800x24, no window manager"

# The XTEST extension probe (the brief's c0 environment evidence; a missing
# extension is STOP 1 — never substituted).
if python3 - "$DISPLAY" << 'PYEOF'
import sys
from Xlib import display as xdisplay
d = xdisplay.Display(sys.argv[1])
ext = d.query_extension("XTEST")
if ext is None or not ext.present:
    print("A-012 BLOCKER: the Xvfb server on %s does not carry the XTEST "
          "extension — input injection is impossible." % sys.argv[1])
    sys.exit(2)
print("A-012 env: XTEST extension present on %s (major opcode %d)"
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
log "A-012 build: cargo build -p pandemonium-client --release"
if ! (cd "$REPO_ROOT" && cargo build -p pandemonium-client --release 2>&1 | tail -3); then
    log "A-012 BLOCKER: the client failed to build."
    exit 2
fi
BIN="$REPO_ROOT/target/release/pandemonium-client"
[ -x "$BIN" ] || { log "A-012 BLOCKER: $BIN missing after the build."; exit 2; }

# ---------------------------------------------------------------------------
# 6. The check sessions (XTEST input, WM_DELETE_WINDOW close, evidence
#    line capture). The driver prints one verdict per check.
# ---------------------------------------------------------------------------
log ""
log "A-012 checks begin (evidence in $EVIDENCE_DIR)"
python3 - "$BIN" "$EVIDENCE_DIR" "$SEED" "$@" << 'PYEOF'
import os, re, subprocess, sys, time
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN, EVID, SEED = sys.argv[1], sys.argv[2], int(sys.argv[3])
FILTERS = sys.argv[4:]

CADENCE = 0.35        # key cadence in seconds (>= 0.25-0.4 s per AI-Handoff §9)
READY_WAIT = 1.0      # settle after the window maps, before the first key
SCENE_WAIT = 1.5      # cutscene seconds before the skip (mid-pan, caption up;
                      # well inside the 6 s scene — a failed skip is still
                      # running at close, so its tick reading is 0)
POST_SKIP = 3.0       # live-match seconds after the skip (c3/c4)
LIVE_RUN = 8.0        # live-match seconds before the Esc (c5-c8: a real
                      # mission match under the pause-menu screens)
POST_RESUME = 4.0     # live-match seconds after a Resume (c7's same-match proof)
TPS = 30              # pandemonium_sim::TICKS_PER_SECOND
T_LIVE_MIN = 60       # >= 2 s of live mission match behind a screen (c3-c8)
                      # — an unpaused-but-never-live reading cannot reach it
T_SAME_MIN = 260      # c7: (LIVE_RUN + POST_RESUME) s of live match at >= 75%
                      # real time; a restart (drop + reconstruct at Resume)
                      # is <= POST_RESUME + cadences old: <= ~140 ticks
T_CUTSCENE_MAX = 2    # c2b: the host frozen by the cutscene (tick 0; an
                      # unpaused match would be ~60 ticks after the 2 s wait)

EXIT_HOST = re.compile(r"exiting at the ([a-z ]+?) \(tick (\d+), (\d+) commands submitted\)")
EXIT_MENU = re.compile(r"exiting at the ([a-z ]+?) \((\d+) match(?:es)? started this run\)")
SMOKE = re.compile(r"windowed smoke: (\d+) frames presented, at the ([a-z ]+?) \(([^)]*)\)")
AUDIO = re.compile(r"— audio: (.+)")

def K(name):  return ("k", name)
def S(sec):   return ("s", sec)
def SHOT(f):  return ("shot", f)
CLOSE = ("close",)

# main menu row 0 (New Match) -> Missions (row 5, after Back — A-011's
# placement keeps every existing row index) -> the picker's Start (row 2).
MENU_TO_PICKER = [K("Enter"), K("Down"), K("Down"), K("Down"), K("Down"), K("Down"), K("Enter")]
PICKER_START  = [K("Down"), K("Down"), K("Enter")]
MISSION_START = MENU_TO_PICKER + PICKER_START
SKIP          = [S(SCENE_WAIT), K("Space")]   # any key ends the scene (§5 (iii))
PAUSE_PREFIX  = MISSION_START + SKIP + [S(LIVE_RUN), K("Escape")]
PAUSE_TO_CONTROLS = [K("Down"), K("Down"), K("Enter")]                     # row 2
PAUSE_TO_QUIT     = [K("Down"), K("Down"), K("Down"), K("Down"), K("Enter")]  # row 4

CHECKS = [
    ("c0", "environment gate: the windowed client presents 60 frames at the main menu",
     [("c0", [], ["--frames", "60"],
       {"smoke": True, "frames": 60, "screen": "main menu", "session": "no match this run"})]),
    ("c1", "main menu -> New Match -> Missions: the mission picker screen is entered",
     [("c1", MENU_TO_PICKER + [CLOSE], [],
       {"screen": "mission picker screen", "matches": 0})]),
    ("c2", "the picker's Mission row cycles (wrap, no crash, the picker stays) and Start opens the cutscene over the paused host",
     [("c2", MENU_TO_PICKER + [K("Left"), K("Right"), CLOSE], [],
       {"screen": "mission picker screen", "matches": 0}),
      ("c2b", MENU_TO_PICKER + [K("Left")] + PICKER_START + [S(2.0), CLOSE], [],
       {"screen": "match", "tick_max": T_CUTSCENE_MAX, "commands": 0})]),
    ("c3", "the cutscene plays and any key skips it: the host unpauses and the match begins",
     [("c3", MISSION_START + [S(SCENE_WAIT), SHOT("c3-cutscene.png"), K("Space"), S(POST_SKIP), CLOSE], [],
       {"screen": "match", "tick_min": T_LIVE_MIN})]),
    ("c4", "the objective HUD rides the live mission match (ticks advancing after the skip; HUD screenshot supplementary)",
     [("c4", MISSION_START + SKIP + [S(POST_SKIP), SHOT("c4-hud.png"), CLOSE], [],
       {"screen": "match", "tick_min": T_LIVE_MIN})]),
    ("c5", "Esc during a mission match opens the pause menu over the live match",
     [("c5", PAUSE_PREFIX + [CLOSE], [],
       {"screen": "pause menu", "tick_min": T_LIVE_MIN})]),
    ("c6", "pause -> Controls is entered over the paused mission match, and Back returns to the pause menu",
     [("c6", PAUSE_PREFIX + PAUSE_TO_CONTROLS + [CLOSE], [],
       {"screen": "controls screen", "tick_min": T_LIVE_MIN}),
      ("c6b", PAUSE_PREFIX + PAUSE_TO_CONTROLS + [K("Enter"), CLOSE], [],
       {"screen": "pause menu", "tick_min": T_LIVE_MIN})]),
    ("c7", "pause -> Resume: the same mission continues (not a restart)",
     [("c7", PAUSE_PREFIX + [K("Enter"), S(POST_RESUME), CLOSE], [],
       {"screen": "match", "tick_min": T_SAME_MIN})]),
    ("c8", "pause -> Quit to Menu opens the Confirm screen; Enter on the default focus (Cancel) keeps the match",
     [("c8", PAUSE_PREFIX + PAUSE_TO_QUIT + [CLOSE], [],
       {"screen": "confirm screen", "tick_min": T_LIVE_MIN}),
      ("c8b", PAUSE_PREFIX + PAUSE_TO_QUIT + [K("Enter"), CLOSE], [],
       {"screen": "pause menu", "tick_min": T_LIVE_MIN})]),
]
CHECKS_BY_NAME = {c[0]: c for c in CHECKS}

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
                raise SystemExit(f"A-012 BLOCKER: no keycode for {name} in the Xvfb keymap")
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
    """The smoke summary line, the exit evidence line (the last one of each
    wins), and the audio arm line. A --frames run prints both the smoke
    summary and the exit line; a driven session prints only the exit line
    (the caller picks by expectation — the A-010 law)."""
    smoke = host = menu = None
    audio = None
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
    return smoke, host, menu, audio

def scan_pixels(path, region):
    """Soft rendered-surface evidence: count the caption/HUD color classes
    inside a region (warm-yellow text, objective green, blue-white text).
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
    smoke, host, menu, audio = parse_evidence(result)
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
        if "tick_min" in expect and (evidence.get("tick") or 0) < expect["tick_min"]:
            errs.append(f"tick {evidence.get('tick')} < {expect['tick_min']}")
        if "tick_max" in expect and (evidence.get("tick")
                                     if evidence.get("tick") is not None
                                     else 1 << 30) > expect["tick_max"]:
            errs.append(f"tick {evidence.get('tick')} > {expect['tick_max']}")
        if "commands" in expect and evidence.get("commands") != expect["commands"]:
            errs.append(f"commands {evidence.get('commands')} != {expect['commands']}")
    return errs, evidence, audio

def fmt_drive(steps):
    return " ".join(step[1] if step[0] == "k" else
                    (f"[{step[1]}s]" if step[0] == "s" else
                     (f"[shot {step[1]}]" if step[0] == "shot" else "[close]"))
                    for step in steps)

drv = Driver()
passed = failed = 0
for name, desc, sessions in CHECKS:
    if FILTERS and name not in FILTERS:
        continue
    print(f"[A-012 {name}] {desc}")
    check_ok = True
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
            if "c3-cutscene" in shot:
                counts = scan_pixels(shot, (150, 420, 650, 590))
                if counts:
                    note = (f" — caption scan {counts} (the warm-yellow title "
                            "line of the subtitle panel, bottom band)")
            if "c4-hud" in shot:
                counts = scan_pixels(shot, (0, 30, 420, 150))
                if counts:
                    note = (" — HUD scan %s (MISSION header warm-yellow, "
                            "[x] line objective-green, [ ] line blue-white, "
                            "top-left block)" % counts)
            print(f"  [{sname}] screenshot: {os.path.basename(shot)}{note}")
        if errs:
            print(f"  [{sname}] verdict: FAIL ({'; '.join(errs)})")
            check_ok = False
        else:
            print(f"  [{sname}] verdict: PASS")
    if check_ok:
        passed += 1
        print(f"[A-012 {name}] verdict: PASS")
    else:
        failed += 1
        print(f"[A-012 {name}] verdict: FAIL")
    print()
    if name == "c0" and not check_ok:
        # The environment gate: every later check needs the windowed client
        # to present frames under Xvfb — STOP here rather than pile up
        # meaningless failures (the brief's STOP 3 shape).
        print("A-012 BLOCKER: the c0 environment gate failed — the client "
              "did not run cleanly under Xvfb (see c0.log). No further "
              "checks were attempted.")
        sys.exit(2)

print(f"=== A-012 Xvfb+XTEST pass: {passed} passed, {failed} failed"
      f" ({passed + failed} checks run) ===")
sys.exit(1 if failed else 0)
PYEOF
DRIVER_RC=$?

log ""
if [ "$DRIVER_RC" -eq 0 ]; then
    log "A-012 RESULT: all checks green. Evidence: $EVIDENCE_DIR"
else
    log "A-012 RESULT: FAILURES PRESENT (driver rc=$DRIVER_RC). Evidence: $EVIDENCE_DIR"
fi
exit "$DRIVER_RC"
