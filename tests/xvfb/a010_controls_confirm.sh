#!/usr/bin/env bash
# A-010 — Xvfb + XTEST machine verification of A-009's two new screens.
#
# Verifies, as rendered surfaces driven by real XTEST input (not as unit
# logic), the Controls screen and the Confirm screen that A-009 added:
#
#   c1/c2  main menu -> Controls -> Back -> main menu
#   c3-c5  live match -> pause menu -> Controls -> Back -> pause menu ->
#          Resume -> the same match continues (tick keeps climbing)
#   c6-c8  pause menu -> Restart -> Confirm screen (Cancel default focus) ->
#          Enter cancels (the match is NOT restarted: still at the pause
#          menu over the same tick) -> the match is still live afterwards
#   c9/c10 pause menu -> Quit to Menu -> Confirm screen -> Esc cancels
#   c11    supplementary: Proceed on the Quit-to-Menu confirm drops the
#          match (pins that row 4's confirm is the Quit confirm)
#   c12    supplementary: Proceed on the Restart confirm restarts (pins
#          that row 3's confirm is the Restart confirm)
#   c0     environment gate: the windowed client presents frames under
#          Xvfb + llvmpipe at the main menu (--frames 60, the summary line)
#
# The recipe is AI-Handoff.md §9 (DEBT-008): Xvfb run directly (no
# xvfb-run, no window manager), keyboard focus via python-xlib
# set_input_focus, input injected via XTEST (python-xlib xtest_fake_input),
# graceful close via the WM_DELETE_WINDOW ClientMessage, wgpu's GL backend
# on llvmpipe (WGPU_BACKEND=gl, LIBGL_ALWAYS_SOFTWARE=1, a userland EGL
# stack when the system one is missing), key cadence >= 0.25-0.4 s.
#
# Each check is one fresh client session: launch under Xvfb, drive the
# keys, send WM_DELETE_WINDOW, capture the exit evidence line
# ("pandemonium client - exiting at the ...") from stdout, and compare it
# against the expected screen / tick / match count. A restart that
# mis-fired would land the client at "the match" with a small fresh tick;
# a cancel lands it at "the pause menu" over the frozen original tick.
#
# Re-runnable: `bash tests/xvfb/a010_controls_confirm.sh [check ...]`
# (no arguments = all checks). Environment knobs:
#   A010_EVIDENCE_DIR  where the per-check logs go
#                      (default: <repo>/target/a010-evidence)
#   A010_USERLAND      the userland libEGL/libXtst tree (default:
#                      $HOME/.local/share/pandemonium/userland-libs; it is
#                      bootstrapped via `apt-get download` + `dpkg -x` if
#                      missing — no root needed)
#   A010_DISPLAY       the Xvfb display number (default 99)
#   A010_SEED          the pre-filled match seed (default 41008)
#
# Never runs the client outside Xvfb. Never modifies source files.

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${A010_EVIDENCE_DIR:-$REPO_ROOT/target/a010-evidence}"
USERLAND="${A010_USERLAND:-$HOME/.local/share/pandemonium/userland-libs}"
DISPLAY_NUM="${A010_DISPLAY:-99}"
SEED="${A010_SEED:-41008}"
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
        log "A-010 BLOCKER: '$tool' is not installed and cannot be substituted."
        exit 2
    }
done
if ! python3 -c "import Xlib" >/dev/null 2>&1; then
    # python-xlib is pure Python and drives XTEST directly (no libXtst
    # needed) — the §9 fallback when xdotool/libXtst are unavailable.
    python3 -m pip install python-xlib >/dev/null 2>&1 \
        || python3 -m pip install --break-system-packages python-xlib >/dev/null 2>&1 \
        || { log "A-010 BLOCKER: python-xlib is not importable and pip cannot install it."; exit 2; }
fi

# ---------------------------------------------------------------------------
# 2. The userland library tree (no root: apt-get download + dpkg -x, per
#    AI-Handoff.md §9 / the A-009 session-start bootstrap): EGL (glvnd +
#    Mesa), libXtst, xkbcommon-x11. Skipped when the system already has
#    everything or the tree exists.
# ---------------------------------------------------------------------------
needs_userland() {
    ldconfig -p 2>/dev/null | grep -q "libEGL.so.1" || return 0
    ldconfig -p 2>/dev/null | grep -q "libXtst.so.6" || return 0
    ldconfig -p 2>/dev/null | grep -q "libxkbcommon-x11.so.0" || return 0
    return 1
}
if [ -d "$USERLAND/usr/lib/x86_64-linux-gnu" ]; then
    log "A-010 env: userland library tree at $USERLAND (pre-existing)"
elif needs_userland; then
    log "A-010 env: bootstrapping the userland library tree at $USERLAND ..."
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
    ) || { log "A-010 BLOCKER: the userland library bootstrap failed (apt-get download / dpkg -x)."; exit 2; }
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
    log "A-010 env: userland library tree bootstrapped at $USERLAND"
else
    log "A-010 env: system EGL/XTEST libraries present; userland tree not needed"
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
    echo "A-010 environment ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
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
    log "A-010 BLOCKER: Xvfb failed to start on $DISPLAY."
    exit 2
fi
log "A-010 env: Xvfb running on $DISPLAY (pid $XVFB_PID), screen 1280x800x24, no window manager"

# ---------------------------------------------------------------------------
# 5. Build the client (the brief's step b).
# ---------------------------------------------------------------------------
log "A-010 build: cargo build -p pandemonium-client --release"
if ! (cd "$REPO_ROOT" && cargo build -p pandemonium-client --release 2>&1 | tail -3); then
    log "A-010 BLOCKER: the client failed to build."
    exit 2
fi
BIN="$REPO_ROOT/target/release/pandemonium-client"
[ -x "$BIN" ] || { log "A-010 BLOCKER: $BIN missing after the build."; exit 2; }

# ---------------------------------------------------------------------------
# 6. The check sessions (XTEST input, WM_DELETE_WINDOW close, evidence
#    line capture). The driver prints one verdict per check.
# ---------------------------------------------------------------------------
log ""
log "A-010 checks begin (evidence in $EVIDENCE_DIR)"
python3 - "$BIN" "$EVIDENCE_DIR" "$SEED" "$@" << 'PYEOF'
import os, re, subprocess, sys, time
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN, EVID, SEED = sys.argv[1], sys.argv[2], int(sys.argv[3])
FILTERS = sys.argv[4:]

CADENCE = 0.35        # key cadence in seconds (>= 0.25-0.4 s per AI-Handoff §9)
READY_WAIT = 1.0      # settle after the window maps, before the first key
PRE_RUN = 15.0        # live-match seconds before the first Esc (>= ~450 ticks)
POST_RESUME = 3.0     # live-match seconds after a Resume (same-match proof)
TPS = 30              # pandemonium_sim::TICKS_PER_SECOND
T_PAUSE_MIN = 300     # >= 10 s of live match behind a pause/confirm screen
T_LIVE_MIN = 400      # >= 13.3 s — only the SAME match can be this old after
                      # a cancel+resume (a restart is <= POST_RESUME+cadences
                      # seconds old: <= ~120 ticks)
T_FRESH_MAX = 150     # a restarted match is a few seconds old at most

EXIT_HOST = re.compile(r"exiting at the ([a-z ]+?) \(tick (\d+), (\d+) commands submitted\)")
EXIT_MENU = re.compile(r"exiting at the ([a-z ]+?) \((\d+) match(?:es)? started this run\)")
SMOKE = re.compile(r"windowed smoke: (\d+) frames presented, at the ([a-z ]+?) \(([^)]*)\)")

def K(name):  return ("k", name)
def S(sec):   return ("s", sec)
CLOSE = ("close",)

# main menu row 0 (New Match) -> Start (row 3) -> 15 s live -> Esc = pause
MATCH_PREFIX = [K("Enter"), K("Down"), K("Down"), K("Down"), K("Enter"), S(PRE_RUN), K("Escape")]
PAUSE_TO_CONTROLS = [K("Down"), K("Down"), K("Enter")]                    # row 2
PAUSE_TO_RESTART  = [K("Down"), K("Down"), K("Down"), K("Enter")]         # row 3
# PLAN-M11 phase 5: the pause menu gained the Resign row (row 4); Quit to
# Menu moved to row 5.
PAUSE_TO_QUIT     = [K("Down"), K("Down"), K("Down"), K("Down"), K("Down"), K("Enter")]  # row 5

CHECKS = [
    ("c0", "environment gate: the windowed client presents 60 frames at the main menu",
     [], ["--frames", "60"],
     {"smoke": True, "frames": 60, "screen": "main menu", "session": "no match this run"}),
    ("c1", "main menu -> Controls: the screen is entered",
     [K("Down"), K("Down"), K("Enter"), CLOSE], [],
     {"screen": "controls screen", "matches": 0}),
    ("c2", "Controls -> Back returns to the main menu",
     [K("Down"), K("Down"), K("Enter"), K("Enter"), CLOSE], [],
     {"screen": "main menu", "matches": 0}),
    ("c3", "live match -> pause -> Controls: the screen is entered over the paused match",
     MATCH_PREFIX + PAUSE_TO_CONTROLS + [CLOSE], [],
     {"screen": "controls screen", "tick_min": T_PAUSE_MIN}),
    ("c4", "Controls -> Back returns to the pause menu",
     MATCH_PREFIX + PAUSE_TO_CONTROLS + [K("Enter"), CLOSE], [],
     {"screen": "pause menu", "tick_min": T_PAUSE_MIN}),
    ("c5", "pause -> Resume returns to the live match (the same match continues)",
     MATCH_PREFIX + PAUSE_TO_CONTROLS + [K("Enter"), K("Enter"), S(POST_RESUME), CLOSE], [],
     {"screen": "match", "tick_min": T_LIVE_MIN}),
    ("c6", "pause -> Restart opens the Confirm screen",
     MATCH_PREFIX + PAUSE_TO_RESTART + [CLOSE], [],
     {"screen": "confirm screen", "tick_min": T_PAUSE_MIN}),
    ("c7", "Confirm: Enter on the default focus (Cancel) cancels — NOT restart",
     MATCH_PREFIX + PAUSE_TO_RESTART + [K("Enter"), CLOSE], [],
     {"screen": "pause menu", "tick_min": T_PAUSE_MIN}),
    ("c8", "after the cancel the match is still live (the same match continues)",
     MATCH_PREFIX + PAUSE_TO_RESTART + [K("Enter"), K("Enter"), S(POST_RESUME), CLOSE], [],
     {"screen": "match", "tick_min": T_LIVE_MIN}),
    ("c9", "pause -> Quit to Menu opens the Confirm screen",
     MATCH_PREFIX + PAUSE_TO_QUIT + [CLOSE], [],
     {"screen": "confirm screen", "tick_min": T_PAUSE_MIN}),
    ("c10", "Confirm: Esc cancels (the match is NOT dropped)",
     MATCH_PREFIX + PAUSE_TO_QUIT + [K("Escape"), CLOSE], [],
     {"screen": "pause menu", "tick_min": T_PAUSE_MIN}),
    ("c11", "supplementary: Proceed on the Quit-to-Menu confirm drops the match",
     MATCH_PREFIX + PAUSE_TO_QUIT + [K("Down"), K("Enter"), CLOSE], [],
     {"screen": "main menu", "matches": 1}),
    ("c12", "supplementary: Proceed on the Restart confirm restarts it (a fresh match)",
     MATCH_PREFIX + PAUSE_TO_RESTART + [K("Down"), K("Enter"), CLOSE], [],
     {"screen": "match", "tick_max": T_FRESH_MAX}),
]

class Driver:
    def __init__(self):
        self.d = display.Display(os.environ.get("DISPLAY"))
        self.root = self.d.screen().root
        self.keycodes = {}
        for name, sym in (("Up", 0xFF52), ("Down", 0xFF54),
                          ("Enter", 0xFF0D), ("Escape", 0xFF1B)):
            kc = self.d.keysym_to_keycode(sym)
            if kc == 0:
                raise SystemExit(f"A-010 BLOCKER: no keycode for {name} in the Xvfb keymap")
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
        if "matches" in expect and evidence.get("matches") != expect["matches"]:
            errs.append(f"matches started {evidence.get('matches')} != {expect['matches']}")
        if "tick_min" in expect and (evidence.get("tick") or 0) < expect["tick_min"]:
            errs.append(f"tick {evidence.get('tick')} < {expect['tick_min']}")
        if "tick_max" in expect and (evidence.get("tick") or 1 << 30) > expect["tick_max"]:
            errs.append(f"tick {evidence.get('tick')} > {expect['tick_max']}")
    drive = " ".join(step[1] if step[0] == "k" else
                     (f"[{step[1]}s]" if step[0] == "s" else "[close]")
                     for step in
                     CHECKS_BY_NAME[name][1]) if name in CHECKS_BY_NAME else ""
    line_ev = next((l for l in reversed(result["lines"])
                    if "exiting at the" in l or "windowed smoke" in l), "(none)")
    print(f"[A-010 {name}] {desc}")
    print(f"  drive: {drive}")
    print(f"  evidence: {line_ev}")
    if errs:
        print(f"  verdict: FAIL ({'; '.join(errs)})")
        return False
    print("  verdict: PASS")
    return True

CHECKS_BY_NAME = {c[0]: (c[0], c[2], c[3], c[4]) for c in CHECKS}
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

print(f"=== A-010 Xvfb+XTEST pass: {passed} passed, {failed} failed"
      f" ({passed + failed} checks run) ===")
sys.exit(1 if failed else 0)
PYEOF
DRIVER_RC=$?

log ""
if [ "$DRIVER_RC" -eq 0 ]; then
    log "A-010 RESULT: all checks green. Evidence: $EVIDENCE_DIR"
else
    log "A-010 RESULT: FAILURES PRESENT (driver rc=$DRIVER_RC). Evidence: $EVIDENCE_DIR"
fi
exit "$DRIVER_RC"
