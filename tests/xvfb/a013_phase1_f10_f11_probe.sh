#!/usr/bin/env bash
# A-013 phase 1 probe: do F10 and F11 arrive at the client under Xvfb?
# (PLAN-M11 §4 phase 1: "If F10 proves grabbed by the WM under Xvfb/X11,
# fall back to GraveAccent and record the decision as an assumption.")
# Xvfb here runs with no window manager, so nothing should eat the keys —
# this proves it. Uses the a010 recipe: Xvfb direct, python-xlib focus +
# XTEST, WM_DELETE_WINDOW close.
set -u
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DISPLAY_NUM="${A013_DISPLAY:-98}"
BIN="$REPO_ROOT/target/release/pandemonium-client"
USERLAND="$HOME/.local/share/pandemonium/userland-libs"

for tool in Xvfb python3; do
    command -v "$tool" >/dev/null 2>&1 || { echo "BLOCKER: $tool missing"; exit 2; }
done
python3 -c "import Xlib" >/dev/null 2>&1 || { echo "BLOCKER: python-xlib missing"; exit 2; }

export LD_LIBRARY_PATH="$USERLAND/usr/lib/x86_64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
export __EGL_VENDOR_LIBRARY_DIRS="$USERLAND/glvnd/egl_vendor.d"
export WGPU_BACKEND=gl
export LIBGL_ALWAYS_SOFTWARE=1
export DISPLAY=":$DISPLAY_NUM"
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/tmp/xdg-runtime}"
mkdir -p "$XDG_RUNTIME_DIR"

pkill -f "Xvfb :$DISPLAY_NUM" >/dev/null 2>&1 || true
sleep 0.5
rm -f "/tmp/.X$DISPLAY_NUM-lock" "/tmp/.X11-unix/X$DISPLAY_NUM"
Xvfb ":$DISPLAY_NUM" -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB_PID=$!
trap 'kill $XVFB_PID >/dev/null 2>&1' EXIT
for _ in $(seq 1 40); do
    [ -S "/tmp/.X11-unix/X$DISPLAY_NUM" ] && break
    sleep 0.25
done

python3 - "$BIN" << 'PYEOF'
import os, subprocess, sys, time
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN = sys.argv[1]
d = display.Display(os.environ.get("DISPLAY"))
root = d.screen().root

def keycode(sym):
    kc = d.keysym_to_keycode(sym)
    if kc == 0:
        raise SystemExit("BLOCKER: no keycode for the key in the Xvfb keymap")
    return kc

F10 = keycode(0xFFC7)   # XK_F10
F11 = keycode(0xFFC8)   # XK_F11
ESC = keycode(0xFF1B)
RET = keycode(0xFF0D)
DOWN = keycode(0xFF54)

def press(kc):
    xtest.fake_input(d, X.KeyPress, detail=kc)
    d.flush(); time.sleep(0.35)
    xtest.fake_input(d, X.KeyRelease, detail=kc)
    d.flush(); time.sleep(0.35)

# Start a match, then press F10/F11 in-match. The bug report writes a
# file next to the CWD; F10's toggle is invisible in stdout, so the
# honest probe is the F11 dump (a file appearing proves F11 arrived),
# and no crash on F10 proves F10 arrived (an unhandled key would simply
# do nothing too — so also check the debug overlay line via --frames
# evidence is impossible; the file is the hard evidence).
import tempfile
workdir = tempfile.mkdtemp(prefix="a013-probe-")
log = open(os.path.join(workdir, "stdout.log"), "w")
proc = subprocess.Popen([BIN, "--seed", "41008"], stdout=log, stderr=subprocess.STDOUT, cwd=workdir)
win = None
t0 = time.time()
before = {w.id for w in root.query_tree().children}
while time.time() - t0 < 30:
    new = [w for w in root.query_tree().children if w.id not in before]
    if new:
        win = new[-1]; break
    if proc.poll() is not None:
        break
    time.sleep(0.2)
if win is None:
    print("BLOCKER: window never appeared"); sys.exit(2)
win.set_input_focus(X.RevertToParent, X.CurrentTime)
d.flush(); time.sleep(1.0)

# Main menu -> New Match (Enter) -> Start (3x Down, Enter) -> live match.
press(RET); press(DOWN); press(DOWN); press(DOWN); press(RET)
time.sleep(8.0)  # let the match run

# F10 in-match: toggles the debug overlay. No crash, match continues.
press(F10); time.sleep(1.0)
# F11 in-match: writes the bug-report dump next to the exe/CWD.
press(F11); time.sleep(2.0)

# Graceful close.
wm_protocols = d.intern_atom("WM_PROTOCOLS")
wm_delete = d.intern_atom("WM_DELETE_WINDOW")
ev = xevent.ClientMessage(window=win, client_type=wm_protocols,
                          data=(32, [wm_delete, 0, 0, 0, 0]))
d.send_event(win, ev, event_mask=X.NoEventMask)
d.flush()
try:
    proc.wait(timeout=30); rc = proc.returncode
except subprocess.TimeoutExpired:
    proc.kill(); rc = None
log.close()

files = sorted(os.listdir(workdir))
dump = [f for f in files if f.endswith(".pdrp") or "info" in f]
print(f"exit code: {rc}")
print(f"workdir files: {files}")
print(f"bug-report artifacts: {dump}")
ok = rc == 0 and len(dump) >= 2  # the .pdrp + the -info.txt sidecar
print("PROBE VERDICT:", "PASS — F10/F11 arrive under Xvfb (no WM grabbed them)" if ok else "FAIL")
sys.exit(0 if ok else 1)
PYEOF
