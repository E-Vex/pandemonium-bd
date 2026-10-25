#!/usr/bin/env bash
# A-013 — Xvfb + XTEST machine verification of the PLAN-M11 Zero Hour
# controls (phases 0-5).
#
# Verifies, as rendered surfaces driven by real XTEST input (not as unit
# logic), the new input paths:
#
#   c0  environment gate: the windowed client presents frames at the menu
#   c1  the camera keys live and submit nothing: numpad 4/6 rotate, 8/2
#       zoom, 5 resets, Ctrl+F1 sets a bookmark, F2 (unset) is inert, F1
#       jumps to the set one — the match runs on, zero commands submitted
#   c2  the selection keys submit nothing: Ctrl+Right selects a unit, E
#       widens to the same kind, Q finds no combat (inert), a click on
#       empty ground deselects — zero commands submitted
#   c3  X scatters: one unit selected -> exactly ONE command submitted
#   c4  G guards: G then a ground click -> exactly ONE command submitted
#   c5  Alt+click waypoints: two Alt-held ground clicks -> exactly TWO
#       commands submitted (replace + queue)
#   c6  F9 toggles the command bar, Tab the (absent) objective panel:
#       no crash, zero commands submitted
#   c7  the minimap drag-pan: press, drag, release over the minimap —
#       the match runs on, zero commands submitted
#   c8  Resign: pause menu row 4 -> confirm -> Proceed submits exactly
#       ONE command and the match reaches the END SCREEN
#
# The recipe is AI-Handoff.md §9 (DEBT-008), the a010 structure: Xvfb run
# directly (no xvfb-run, no window manager), keyboard focus via
# python-xlib set_input_focus, input injected via XTEST (keys AND pointer
# motion + buttons for the mouse paths), graceful close via the
# WM_DELETE_WINDOW ClientMessage, key cadence >= 0.25-0.4 s under llvmpipe.
#
# Re-runnable: `bash tests/xvfb/a013_zh_controls.sh [check ...]`
# (no arguments = all checks). Environment knobs:
#   A013_EVIDENCE_DIR  where the per-check logs go
#                      (default: <repo>/target/a013-evidence)
#   A013_USERLAND      the userland libEGL/libXtst tree (default:
#                      $HOME/.local/share/pandemonium/userland-libs; it is
#                      bootstrapped via `apt-get download` + `dpkg -x` if
#                      missing — no root needed)
#   A013_DISPLAY       the Xvfb display number (default 97)
#   A013_SEED          the pre-filled match seed (default 41013)
#
# Never runs the client outside Xvfb. Never modifies source files.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${A013_EVIDENCE_DIR:-$REPO_ROOT/target/a013-evidence}"
USERLAND="${A013_USERLAND:-$HOME/.local/share/pandemonium/userland-libs}"
DISPLAY_NUM="${A013_DISPLAY:-97}"
SEED="${A013_SEED:-41013}"
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
        log "A-013 BLOCKER: '$tool' is not installed and cannot be substituted."
        exit 2
    }
done
if ! python3 -c "import Xlib" >/dev/null 2>&1; then
    python3 -m pip install python-xlib >/dev/null 2>&1 \
        || python3 -m pip install --break-system-packages python-xlib >/dev/null 2>&1 \
        || { log "A-013 BLOCKER: python-xlib is not importable and pip cannot install it."; exit 2; }
fi

# ---------------------------------------------------------------------------
# 2. The userland library tree (no root: apt-get download + dpkg -x, per
#    AI-Handoff.md §9). Skipped when the system already has everything.
# ---------------------------------------------------------------------------
needs_userland() {
    ldconfig -p 2>/dev/null | grep -q "libEGL.so.1" || return 0
    ldconfig -p 2>/dev/null | grep -q "libXtst.so.6" || return 0
    ldconfig -p 2>/dev/null | grep -q "libxkbcommon-x11.so.0" || return 0
    return 1
}
if [ -d "$USERLAND/usr/lib/x86_64-linux-gnu" ]; then
    log "A-013 env: userland library tree at $USERLAND (pre-existing)"
elif needs_userland; then
    log "A-013 env: bootstrapping the userland library tree at $USERLAND ..."
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
    ) || { log "A-013 BLOCKER: the userland library bootstrap failed (apt-get download / dpkg -x)."; exit 2; }
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
    log "A-013 env: userland library tree bootstrapped at $USERLAND"
else
    log "A-013 env: system EGL/XTEST libraries present; userland tree not needed"
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
# 3. Environment evidence.
# ---------------------------------------------------------------------------
{
    echo "A-013 environment ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
    echo "Xvfb: $(command -v Xvfb)$(dpkg -l xvfb 2>/dev/null | awk '/^ii/ {print " ("$3")"}')"
    echo "python: $(python3 --version 2>&1), python-xlib: $(python3 -c 'import Xlib; print(Xlib.__version__)' 2>&1)"
    echo "XTEST injection: python-xlib xtest_fake_input (keys, pointer motion, buttons)"
    echo "renderer: WGPU_BACKEND=$WGPU_BACKEND LIBGL_ALWAYS_SOFTWARE=$LIBGL_ALWAYS_SOFTWARE (Mesa llvmpipe)"
    echo "llvmpipe driver: $(ls /usr/lib/x86_64-linux-gnu/dri/swrast_dri.so 2>/dev/null || echo 'MISSING')"
    [ -n "$USERLAND" ] && echo "userland tree: $USERLAND" || echo "userland tree: not needed (system EGL)"
    echo "seed: $SEED, display: $DISPLAY"
    echo
} | tee "$EVIDENCE_DIR/env.txt"

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
    log "A-013 BLOCKER: Xvfb failed to start on $DISPLAY."
    exit 2
fi
log "A-013 env: Xvfb running on $DISPLAY (pid $XVFB_PID), screen 1280x800x24, no window manager"

# ---------------------------------------------------------------------------
# 5. Build the client.
# ---------------------------------------------------------------------------
log "A-013 build: cargo build -p pandemonium-client --release"
if ! (cd "$REPO_ROOT" && cargo build -p pandemonium-client --release 2>&1 | tail -3); then
    log "A-013 BLOCKER: the client failed to build."
    exit 2
fi
BIN="$REPO_ROOT/target/release/pandemonium-client"
[ -x "$BIN" ] || { log "A-013 BLOCKER: $BIN missing after the build."; exit 2; }

# ---------------------------------------------------------------------------
# 6. The check sessions.
# ---------------------------------------------------------------------------
log ""
log "A-013 checks begin (evidence in $EVIDENCE_DIR)"
python3 - "$BIN" "$EVIDENCE_DIR" "$SEED" "$@" << 'PYEOF'
import os, re, subprocess, sys, time
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN, EVID, SEED = sys.argv[1], sys.argv[2], int(sys.argv[3])
FILTERS = sys.argv[4:]

CADENCE = 0.35        # key cadence in seconds (>= 0.25-0.4 s per AI-Handoff §9)
READY_WAIT = 1.0      # settle after the window maps, before the first key
PRE_RUN = 8.0         # live-match seconds before the first input
POST_INPUT = 3.0      # live-match seconds after the inputs (evidence tail)
TPS = 30              # pandemonium_sim::TICKS_PER_SECOND

EXIT_HOST = re.compile(r"exiting at the ([a-z ]+?) \(tick (\d+), (\d+) commands submitted\)")
EXIT_MENU = re.compile(r"exiting at the ([a-z ]+?) \((\d+) match(?:es)? started this run\)")
SMOKE = re.compile(r"windowed smoke: (\d+) frames presented, at the ([a-z ]+?) \(([^)]*)\)")

def K(name):  return ("k", name)
def KD(name): return ("kd", name)               # key down (modifier hold begins)
def KU(name): return ("ku", name)               # key up (modifier hold ends)
def M(x, y):  return ("m", x, y)                # pointer motion
def C(button): return ("c", button)             # click press+release at current position
def P(button): return ("p", button)             # press only
def R(button): return ("r", button)             # release only
def S(sec):    return ("s", sec)
CLOSE = ("close",)

# Main menu row 0 (New Match) -> Start (row 3) -> live match.
MATCH_PREFIX = [K("Enter"), K("Down"), K("Down"), K("Down"), K("Enter"), S(PRE_RUN)]
# ...and one unit selected (Ctrl+Right cycles to the lowest-id own unit —
# a worker on the Alpha content's starting force).
SELECT_ONE = [KD("Control_L"), K("Right"), KU("Control_L")]

CHECKS = [
    ("c0", "environment gate: the windowed client presents 60 frames at the main menu",
     [], ["--frames", "60"],
     {"smoke": True, "frames": 60, "screen": "main menu", "session": "no match this run"}),
    ("c1", "the camera keys live and submit nothing (numpad rotate/zoom/reset, bookmarks)",
     MATCH_PREFIX + [
         K("Numpad4"), K("Numpad6"),          # rotate
         K("Numpad8"), K("Numpad2"),          # zoom
         K("Numpad5"),                        # reset
         KD("Control_L"),                     # hold Ctrl...
         K("F1"),                             # ...F1 sets bookmark 1
         K("F2"),                             # (still held) F2 sets bookmark 2
         KU("Control_L"),                     # release Ctrl
         K("F1"),                             # jump to bookmark 1
         K("F2"),                             # jump to bookmark 2
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 0, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c2", "the selection keys submit nothing (Ctrl+arrow cycle, E, Q, empty-click deselect)",
     MATCH_PREFIX + SELECT_ONE + [
         K("E"),                              # same-kind on screen
         K("Q"),                              # no combat at the opening: inert
         M(700, 420), C(1),                  # empty ground: deselect
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 0, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c3", "X scatters: one selected unit -> exactly one command",
     MATCH_PREFIX + SELECT_ONE + [
         K("X"),                              # scatter it
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 1, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c4", "G guards: G then a ground click -> exactly one command",
     MATCH_PREFIX + SELECT_ONE + [
         K("G"),                              # arm guard
         M(700, 420), C(1),                   # the armed click: a plain Move
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 1, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c5", "Alt+click waypoints: two Alt-held clicks -> exactly two commands",
     MATCH_PREFIX + SELECT_ONE + [
         KD("Alt_L"),                         # hold Alt for the gesture
         M(650, 400), C(1),                   # first click: replaces
         M(700, 450), C(1),                   # second click: queues
         KU("Alt_L"),                         # the release sends nothing
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 2, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c6", "F9 toggles the command bar and Tab the objective panel: no crash, no commands",
     MATCH_PREFIX + [
         K("F9"),                             # hide the bar (minimap included)
         S(1.0),
         M(60, 540), C(1),                    # a click where the minimap was:
                                             # falls through to the world
         K("Tab"),                            # plain match: inert
         K("F9"),                             # bar back
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 0, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c7", "the minimap drag-pan: press, drag, release over the minimap",
     MATCH_PREFIX + [
         M(60, 540), P(1),                    # press on the minimap
         M(150, 560), M(100, 520),            # drag: every move refocuses
         R(1),                                # release ends the drag
         S(POST_INPUT), CLOSE,
     ], [],
     {"screen": "match", "commands": 0, "tick_min": PRE_RUN * TPS * 0.8}),
    ("c8", "Resign: pause row 4 -> confirm -> Proceed: one command, the END SCREEN",
     MATCH_PREFIX + [
         K("Escape"),                         # pause menu
         K("Down"), K("Down"), K("Down"), K("Down"),  # row 4: Resign
         K("Enter"),                          # open the confirm screen
         K("Down"), K("Enter"),               # row 1: Confirm (Proceed)
         S(4.0),                              # the resign resolves, the end screen
     CLOSE,
     ], [],
     {"screen": "end screen", "commands": 1}),
]

class Driver:
    def __init__(self):
        self.d = display.Display(os.environ.get("DISPLAY"))
        self.root = self.d.screen().root
        self.keycodes = {}
        for name, sym in (("Up", 0xFF52), ("Down", 0xFF54),
                          ("Enter", 0xFF0D), ("Escape", 0xFF1B),
                          ("Right", 0xFF53), ("Left", 0xFF51),
                          ("F1", 0xFFBE), ("F2", 0xFFBF),
                          ("F9", 0xFFC6),
                          ("Numpad4", 0xFFB4), ("Numpad6", 0xFFB6),
                          ("Numpad8", 0xFFB8), ("Numpad2", 0xFFB2),
                          ("Numpad5", 0xFFB5),
                          ("Control_L", 0xFFE3), ("Alt_L", 0xFFE9),
                          ("E", 0x45), ("Q", 0x51), ("X", 0x58), ("G", 0x47),
                          ("Tab", 0xFF09)):
            kc = self.d.keysym_to_keycode(sym)
            if kc == 0:
                raise SystemExit(f"A-013 BLOCKER: no keycode for {name} in the Xvfb keymap")
            self.keycodes[name] = kc
        # XTEST needs the modifier keys ENABLED in the server's modifier
        # map for the client to see the modifier state; the default Xvfb
        # keymap already carries Control_L/Alt_L as modifiers.
        self.wm_protocols = self.d.intern_atom("WM_PROTOCOLS")
        self.wm_delete = self.d.intern_atom("WM_DELETE_WINDOW")

    def key(self, name, hold=None):
        kc = self.keycodes[name]
        xtest.fake_input(self.d, X.KeyPress, detail=kc)
        self.d.flush()
        time.sleep(hold if hold else CADENCE)
        xtest.fake_input(self.d, X.KeyRelease, detail=kc)
        self.d.flush()
        time.sleep(CADENCE)

    def key_down(self, name):
        xtest.fake_input(self.d, X.KeyPress, detail=self.keycodes[name])
        self.d.flush()
        time.sleep(CADENCE)

    def key_up(self, name):
        xtest.fake_input(self.d, X.KeyRelease, detail=self.keycodes[name])
        self.d.flush()
        time.sleep(CADENCE)

    def motion(self, x, y):
        # Move the pointer stepwise (XTEST MotionNotify jumps are fine).
        xtest.fake_input(self.d, X.MotionNotify, x=x, y=y)
        self.d.flush()
        time.sleep(CADENCE)

    def button(self, btn, press):
        xtest.fake_input(self.d, X.ButtonPress if press else X.ButtonRelease,
                         detail=btn)
        self.d.flush()
        time.sleep(CADENCE)

    def send_close(self, win):
        ev = xevent.ClientMessage(window=win, client_type=self.wm_protocols,
                                  data=(32, [self.wm_delete, 0, 0, 0, 0]))
        self.d.send_event(win, ev, event_mask=X.NoEventMask)
        self.d.flush()

    def run(self, name, steps, extra_args):
        """One session: launch, drive, close, return the parsed evidence."""
        log_path = os.path.join(EVID, name + ".log")
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
                elif step[0] == "kd" and win is not None:
                    self.key_down(step[1])
                elif step[0] == "ku" and win is not None:
                    self.key_up(step[1])
                elif step[0] == "m" and win is not None:
                    self.motion(step[1], step[2])
                elif step[0] == "c" and win is not None:
                    self.button(step[1], True)
                    self.button(step[1], False)
                elif step[0] == "p" and win is not None:
                    self.button(step[1], True)
                elif step[0] == "r" and win is not None:
                    self.button(step[1], False)
                elif step[0] == "s":
                    time.sleep(step[1])
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
        return {"rc": rc, "lines": lines}

def verdict(name, desc, result, expect):
    errs = []
    evidence = None
    for line in result["lines"]:
        if expect.get("smoke"):
            m = SMOKE.search(line)
            if m:
                evidence = {"kind": "smoke", "frames": int(m.group(1)),
                            "screen": m.group(2), "session": m.group(3)}
        else:
            m = EXIT_HOST.search(line)
            if m:
                evidence = {"kind": "host", "screen": m.group(1),
                            "tick": int(m.group(2)), "commands": int(m.group(3))}
            m = EXIT_MENU.search(line)
            if m:
                evidence = {"kind": "menu", "screen": m.group(1),
                            "matches": int(m.group(2))}
    if result["rc"] not in (0,):
        errs.append(f"client exit code {result['rc']} (expected a clean 0)")
    if evidence is None:
        errs.append("no exit evidence line in the session log")
    else:
        if expect.get("smoke"):
            if evidence["kind"] != "smoke":
                errs.append("no --frames windowed-smoke summary line")
            if evidence.get("frames") != expect["frames"]:
                errs.append(f"frames {evidence.get('frames')} != {expect['frames']}")
            if expect.get("session") and expect["session"] not in evidence.get("session", ""):
                errs.append(f"session '{evidence.get('session')}' lacks '{expect['session']}'")
        if "screen" in expect and evidence.get("screen") != expect["screen"]:
            errs.append(f"screen '{evidence.get('screen')}' != '{expect['screen']}'")
        if "commands" in expect and evidence.get("commands") != expect["commands"]:
            errs.append(f"commands submitted {evidence.get('commands')} != {expect['commands']}")
        if "tick_min" in expect and (evidence.get("tick") or 0) < expect["tick_min"]:
            errs.append(f"tick {evidence.get('tick')} < {expect['tick_min']}")
    line_ev = next((l for l in reversed(result["lines"])
                    if "exiting at the" in l or "windowed smoke" in l), "(none)")
    print(f"[A-013 {name}] {desc}")
    print(f"  evidence: {line_ev}")
    if errs:
        print(f"  verdict: FAIL ({'; '.join(errs)})")
        return False
    print("  verdict: PASS")
    return True

drv = Driver()
passed = failed = 0
for name, desc, steps, extra_args, expect in CHECKS:
    if FILTERS and name not in FILTERS:
        continue
    result = drv.run(name, steps, extra_args)
    if verdict(name, desc, result, expect):
        passed += 1
    else:
        failed += 1
    print()

print(f"=== A-013 Xvfb+XTEST pass: {passed} passed, {failed} failed"
      f" ({passed + failed} checks run) ===")
sys.exit(1 if failed else 0)
PYEOF
DRIVER_RC=$?

log ""
if [ "$DRIVER_RC" -eq 0 ]; then
    log "A-013 RESULT: all checks green. Evidence: $EVIDENCE_DIR"
else
    log "A-013 RESULT: FAILURES PRESENT (driver rc=$DRIVER_RC). Evidence: $EVIDENCE_DIR"
fi
exit "$DRIVER_RC"
