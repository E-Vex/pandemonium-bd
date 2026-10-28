#!/usr/bin/env bash
# A-020 — Xvfb + XTEST machine verification of the model gallery + the
# visibility fix (M12 phase 5b).
#
# Verifies, against the real windowed client on real packs:
#
#   c1  the gallery runs on the OFFICIAL pack (the dev-layout discovery, no
#       --pack): schema-1 line, the eight skins load, all nine kinds present
#       (one gallery frame per kind, each a live render), Esc exits 0
#   c2  the gallery runs on the PILOT pack (--pack): three model lines, six
#       skin lines, exactly ONE diagnostic per corrupted kind (A-146), the
#       keys work (T flips the TEAM text, Up/Down zoom, Esc exits 0)
#   c3  THE PIXEL TEST per model kind (the brief's acceptance b): the pilot
#       frame vs the corrupted-model frame at the same kind — the model's
#       pixels are the difference, every differing pixel lands inside the
#       expected screen rects (the kind rect + the stats-text rect), zero
#       outside, and the per-kind minimums hold — with the numbers printed
#   c4  the tier label follows the loaded pack: rifleman reads model on the
#       pilot pack and skin on the official pack, ore_node reads model on
#       the pilot and placeholder on the official (the official pack ships
#       no ore_node entry) — the label row's pixels differ between the packs
#   c5  the report frames: a spectated match on the pilot pack at the default
#       camera — 4x crops of an ore node and a rifleman group — plus the
#       gallery frames of the same kinds, into docs/pm/reports/a020-frames/
#
# The recipe is AI-Handoff.md §9 (DEBT-008), the a018/a013 structure: Xvfb
# run directly (no xvfb-run, no window manager), keyboard focus via
# python-xlib set_input_focus, input via XTEST, frame capture via get_image,
# graceful close via the WM_DELETE_WINDOW ClientMessage.
#
# Re-runnable: `bash tests/xvfb/a020_gallery.sh [check ...]`
# (no arguments = all checks). Environment knobs:
#   A020_EVIDENCE_DIR  where the per-check logs and frames go
#                      (default: <repo>/target/a020-evidence)
#   A020_USERLAND      the userland libEGL/libXtst tree (default:
#                      $HOME/.local/share/pandemonium/userland-libs;
#                      bootstrapped via apt-get download + dpkg -x if
#                      missing — no root needed)
#   A020_DISPLAY       the Xvfb display number (default 96)
#   A020_ASSETS_REPO   override for the assets repo URL (default: the
#                      official one; a local clone works too)
#
# Never runs the client outside Xvfb. Never modifies source files (the pilot
# packs are built OUTSIDE the repository; the report frames folder is the
# brief's declared deliverable).

set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
EVIDENCE_DIR="${A020_EVIDENCE_DIR:-$REPO_ROOT/target/a020-evidence}"
USERLAND="${A020_USERLAND:-$HOME/.local/share/pandemonium/userland-libs}"
DISPLAY_NUM="${A020_DISPLAY:-96}"
ASSETS_REPO="${A020_ASSETS_REPO:-}"
PILOT_DIR="${A020_PILOT_OUT:-/tmp/pandemonium-a020-pilot}"
BAD_RIFLE="${A020_BAD_RIFLE_OUT:-/tmp/pandemonium-a020-bad-rifleman}"
BAD_BARRACKS="${A020_BAD_BARRACKS_OUT:-/tmp/pandemonium-a020-bad-barracks}"
BAD_NODE="${A020_BAD_NODE_OUT:-/tmp/pandemonium-a020-bad-ore-node}"
FRAMES_DIR="$REPO_ROOT/docs/pm/reports/a020-frames"
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
        log "A-020 BLOCKER: '$tool' is not installed and cannot be substituted."
        exit 2
    }
done
if ! python3 -c "import Xlib" >/dev/null 2>&1; then
    python3 -m pip install python-xlib >/dev/null 2>&1 \
        || python3 -m pip install --break-system-packages python-xlib >/dev/null 2>&1 \
        || { log "A-020 BLOCKER: python-xlib is not importable and pip cannot install it."; exit 2; }
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
    log "A-020 env: userland library tree at $USERLAND (pre-existing)"
elif needs_userland; then
    log "A-020 env: bootstrapping the userland library tree at $USERLAND ..."
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
    ) || { log "A-020 BLOCKER: the userland library bootstrap failed."; exit 2; }
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
    log "A-020 env: userland library tree bootstrapped at $USERLAND"
else
    log "A-020 env: system EGL/XTEST libraries present; userland tree not needed"
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
    echo "A-020 environment ($(date -u +%Y-%m-%dT%H:%M:%SZ))"
    echo "python: $(python3 --version 2>&1), python-xlib: $(python3 -c 'import Xlib; print(Xlib.__version__)' 2>&1)"
    echo "renderer: WGPU_BACKEND=$WGPU_BACKEND LIBGL_ALWAYS_SOFTWARE=$LIBGL_ALWAYS_SOFTWARE (Mesa llvmpipe)"
    echo "display: $DISPLAY, evidence: $EVIDENCE_DIR"
    echo
} | tee "$EVIDENCE_DIR/env.txt"

# ---------------------------------------------------------------------------
# 3. Build the client (release), the official pack, and the pilot variants.
# ---------------------------------------------------------------------------
log "A-020 build: cargo build -p pandemonium-client --release"
if ! (cd "$REPO_ROOT" && cargo build -p pandemonium-client --release 2>&1 | tail -2); then
    log "A-020 BLOCKER: the client failed to build."
    exit 2
fi
BIN="$REPO_ROOT/target/release/pandemonium-client"
[ -x "$BIN" ] || { log "A-020 BLOCKER: $BIN missing after the build."; exit 2; }

log "A-020 packs: fetching the official pack into the dev layout"
FETCH=(bash "$REPO_ROOT/scripts/fetch-assets.sh")
[ -n "$ASSETS_REPO" ] && FETCH=(env PANDEMONIUM_ASSETS_REPO="$ASSETS_REPO" "${FETCH[@]}")
"${FETCH[@]}" >/dev/null 2>&1 \
    || { log "A-020 BLOCKER: fetch-assets.sh failed (c1 needs the official pack)."; exit 2; }
[ -f "$REPO_ROOT/crates/client/assets/packs/default/manifest.ron" ] \
    || { log "A-020 BLOCKER: the official pack did not install into the dev layout."; exit 2; }

log "A-020 packs: building the pilot pack variants (outside the repository)"
MAKE_PACK=(bash "$REPO_ROOT/scripts/make-pilot-pack.sh")
[ -n "$ASSETS_REPO" ] && MAKE_PACK=(env PANDEMONIUM_ASSETS_REPO="$ASSETS_REPO" "${MAKE_PACK[@]}")
PILOT_OUT="$PILOT_DIR" "${MAKE_PACK[@]}" >/dev/null 2>&1 \
    || { log "A-020 BLOCKER: make-pilot-pack.sh failed."; exit 2; }
rm -rf "$BAD_RIFLE" "$BAD_BARRACKS" "$BAD_NODE"
cp -R "$PILOT_DIR" "$BAD_RIFLE"
cp -R "$PILOT_DIR" "$BAD_BARRACKS"
cp -R "$PILOT_DIR" "$BAD_NODE"
printf 'not a glb: A-020 corrupt-rifleman fixture for the gallery pixel test' \
    > "$BAD_RIFLE/models/units/rifleman.glb"
printf 'not a glb: A-020 corrupt-barracks fixture for the gallery pixel test' \
    > "$BAD_BARRACKS/models/buildings/barracks.glb"
printf 'not a glb: A-020 corrupt-ore-node fixture for the gallery pixel test' \
    > "$BAD_NODE/models/nodes/ore_node.glb"
log "A-020 packs: pilot at $PILOT_DIR; bad-rifle at $BAD_RIFLE; bad-barracks at $BAD_BARRACKS; bad-node at $BAD_NODE"

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
    log "A-020 BLOCKER: Xvfb failed to start on $DISPLAY."
    exit 2
fi
log "A-020 env: Xvfb running on $DISPLAY (pid $XVFB_PID), screen 1280x800x24, no window manager"

# ---------------------------------------------------------------------------
# 5. The checks.
# ---------------------------------------------------------------------------
log ""
log "A-020 checks begin (evidence in $EVIDENCE_DIR)"
python3 - "$BIN" "$EVIDENCE_DIR" "$PILOT_DIR" "$BAD_RIFLE" "$BAD_BARRACKS" "$BAD_NODE" "$FRAMES_DIR" "$REPO_ROOT" "$@" << 'PYEOF'
import os, re, subprocess, sys, time, zlib, struct
from pathlib import Path
from Xlib import X, display
from Xlib.ext import xtest
from Xlib.protocol import event as xevent

BIN, EVID, PILOT, BAD_RIFLE, BAD_BARRACKS, BAD_NODE, FRAMES, REPO = sys.argv[1:9]
FILTERS = sys.argv[9:]

SMOKE = re.compile(r"windowed smoke: (\d+) frames presented")
PACK_LINE = re.compile(r"pandemonium client — asset pack '([a-z_0-9]+)' \(schema (\d+)\) loaded")
MODEL_LINE = re.compile(r"pandemonium client — entity model '([a-z_]+)' loaded")
SKIN_LINE = re.compile(r"pandemonium client — entity skin '([a-z_]+)' loaded")
NO_PACK = re.compile(r"no asset pack manifest")

W, H = 1280, 800
WIN_W, WIN_H = 800, 600
# The pixel test's expected screen rects (window coordinates, calibrated on
# the llvmpipe captures): the kind rect around the world center, the stats
# text block at the top-left. Every differing pixel must land in one of
# them; the numbers are printed either way.
KIND_RECT = (176, 112, 592, 464)
TEXT_RECT = (0, 14, 640, 100)
# The per-kind minimum differing pixels INSIDE the kind rect — each kind's
# model must actually draw there (calibrated at ~1/3..1/5 of the observed
# counts so wall-clock turntable phase cannot starve a pass).
KIND_MINIMUMS = {"barracks": 10000, "ore_node": 3000, "rifleman": 800}
# The gallery's kind order = content order = the capture order.
KIND_NAMES = ["barracks", "command_center", "guardian", "ore_node", "raider",
              "rifleman", "supply_depot", "turret", "worker"]
MODEL_KINDS = {0: "barracks", 3: "ore_node", 5: "rifleman"}


def write_png(path, raw_bgra, w, h):
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
    Path(path).write_bytes(png)


def read_png(path):
    data = Path(path).read_bytes()
    off, w, h, idat = 8, 0, 0, b""
    while off < len(data):
        clen = struct.unpack_from(">I", data, off)[0]
        tag = data[off+4:off+8]
        chunk = data[off+8:off+8+clen]
        off += 12 + clen
        if tag == b"IHDR":
            w, h, depth, ctype = struct.unpack_from(">IIBB", chunk)
            assert depth == 8 and ctype == 2, "the machine pass captures RGB PNGs"
        elif tag == b"IDAT":
            idat += chunk
    raw = zlib.decompress(idat)
    stride = w * 3 + 1
    px = [None] * (w * h)
    for y in range(h):
        row = raw[y*stride+1:(y+1)*stride]
        for x in range(w):
            i = x * 3
            px[y*w+x] = (row[i], row[i+1], row[i+2])
    return w, h, px


def crop_4x(src_path, x0, y0, cw, ch, out_path):
    """Crops the 800x600 window and 4x-enlarges (nearest) into out_path."""
    w, h, px = read_png(src_path)
    crop = [[px[(y0+dy)*w + x] for x in range(x0, min(x0+cw, w))]
            for dy in range(ch) if y0+dy < h]
    big_rows = []
    for row in crop:
        big_row = []
        for p in row:
            big_row.extend([p] * 4)
        for _ in range(4):
            big_rows.append(big_row)
    rows = bytearray()
    for row in big_rows:
        rows += b"\x00" + b"".join(struct.pack(">BBB", *p) for p in row)
    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", len(big_rows[0]), len(big_rows), 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 6))
    png += chunk(b"IEND", b"")
    Path(out_path).write_bytes(png)
    print(f"A-020 frame: {out_path} ({len(big_rows[0])}x{len(big_rows)}, from {x0},{y0} {cw}x{ch} x4)")


def pixel_diff(a_path, b_path):
    """Returns (total, in_kind, in_text, outside, sample) for the window."""
    wa, ha, pa = read_png(a_path)
    _, _, pb = read_png(b_path)
    total = in_kind = in_text = outside = 0
    sample = None
    for y in range(WIN_H):
        for x in range(WIN_W):
            ra, ga, ba = pa[y*wa+x]
            rb, gb, bb = pb[y*wa+x]
            if abs(ra-rb)+abs(ga-gb)+abs(ba-bb) > 30:
                total += 1
                if KIND_RECT[0] <= x <= KIND_RECT[2] and KIND_RECT[1] <= y <= KIND_RECT[3]:
                    in_kind += 1
                elif TEXT_RECT[0] <= x <= TEXT_RECT[2] and TEXT_RECT[1] <= y <= TEXT_RECT[3]:
                    in_text += 1
                else:
                    outside += 1
                    if sample is None:
                        sample = (x, y)
    return total, in_kind, in_text, outside, sample


def live_variance(path):
    """A live render has pixel variance; a void screen is a flat buffer."""
    w, h, px = read_png(path)
    sample = [px[i] for i in range(0, w*WIN_H, 997)]
    return len(set(sample)) >= 8


d = display.Display(os.environ.get("DISPLAY"))
root = d.screen().root
SYMS = {"Return": 0xFF0D, "Right": 0xFF53, "Left": 0xFF51, "Up": 0xFF52,
        "Down": 0xFF54, "Escape": 0xFF1B, "T": 0x74}
keycodes = {}
for name, sym in SYMS.items():
    kc = d.keysym_to_keycode(sym)
    if kc == 0:
        print(f"A-020 BLOCKER: no keycode for {name}")
        sys.exit(2)
    keycodes[name] = kc


def key(name, hold=0.3, after=0.3):
    xtest.fake_input(d, X.KeyPress, detail=keycodes[name])
    d.flush(); time.sleep(hold)
    xtest.fake_input(d, X.KeyRelease, detail=keycodes[name])
    d.flush(); time.sleep(after)


def grab(path):
    raw = root.get_image(0, 0, W, H, X.ZPixmap, 0xffffffff).data
    write_png(path, raw, W, H)


def close(window):
    protocols = d.intern_atom("WM_PROTOCOLS", False)
    delete = d.intern_atom("WM_DELETE_WINDOW", False)
    ev = xevent.ClientMessage(window=window, client_type=protocols,
                              data=(32, [int(delete), X.CurrentTime, 0, 0, 0]))
    window.send_event(ev)
    d.sync()


class Client:
    """One windowed gallery session: launch, find the window, focus it."""
    def __init__(self, args, log_name):
        self.log_path = os.path.join(EVID, log_name)
        self.log = open(self.log_path, "w")
        self.proc = subprocess.Popen([BIN] + args, stdout=self.log,
                                     stderr=subprocess.STDOUT)
        before = {w.id for w in root.query_tree().children}
        self.win = None
        t0 = time.time()
        while time.time() - t0 < 30:
            new = [w for w in root.query_tree().children if w.id not in before]
            if new:
                self.win = new[-1]
                break
            if self.proc.poll() is not None:
                break
            time.sleep(0.2)
        if self.win is not None:
            self.win.set_input_focus(X.RevertToParent, X.CurrentTime)
            d.flush()
            time.sleep(4.0)   # the window maps and the first frames present

    def gallery_frames(self, tag):
        """Captures one frame per kind, pressing Right between them."""
        frames = []
        for i in range(9):
            path = os.path.join(EVID, f"{tag}-kind-{i:02d}.png")
            grab(path)
            frames.append(path)
            key("Right")
        return frames

    def log_text(self):
        self.log.close()
        with open(self.log_path) as f:
            return f.read()

    def wait_exit(self, timeout=25):
        try:
            self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return False
        return self.proc.returncode == 0

    def kill(self):
        if self.proc.poll() is None:
            self.proc.kill()


def check(name, description):
    if FILTERS and name not in FILTERS:
        return False
    print(f"A-020 {name}: {description}")
    return True


def report(name, errs):
    if errs:
        print(f"A-020 {name}: FAIL")
        for err in errs:
            print(f"  - {err}")
        return False
    print(f"A-020 {name}: PASS")
    return True


results = []

# c1 — the gallery on the OFFICIAL pack (the dev-layout discovery).
if check("c1", "the gallery runs on the official pack: schema 1, eight skins, nine live kind frames, Esc exits 0"):
    client = Client(["--gallery"], "c1-official.log")
    errs = []
    if client.win is None:
        errs.append("the client window never appeared")
        client.kill()
        client.log_text()
    else:
        frames = client.gallery_frames("c1")
        key("Escape")
        ok = client.wait_exit()
        out = client.log_text()
        m = PACK_LINE.search(out)
        if not m or m.group(1) != "tactical_field" or m.group(2) != "1":
            errs.append(f"the official pack line is wrong: {m}")
        skins = sorted(SKIN_LINE.findall(out))
        if skins != ["barracks", "command_center", "guardian", "raider",
                     "rifleman", "supply_depot", "turret", "worker"]:
            errs.append(f"the official pack's eight skins did not all load: {skins}")
        if MODEL_LINE.search(out):
            errs.append("the official (schema 1) pack loaded a model — it has none")
        for i, frame in enumerate(frames):
            if not live_variance(frame):
                errs.append(f"{KIND_NAMES[i]}'s official-pack frame looks void")
                break
        if not ok:
            errs.append("Esc did not exit the gallery cleanly")
        print(f"A-020 c1: nine frames captured, {len(skins)} skins, exit code 0: {ok}")
    results.append(report("c1", errs))

# c2 — the gallery on the PILOT pack + the keys + the A-146 pin.
if check("c2", "the gallery runs on the pilot pack: three models, six skins, keys work, Esc exits 0"):
    client = Client(["--gallery", "--pack", PILOT], "c2-pilot.log")
    errs = []
    if client.win is None:
        errs.append("the client window never appeared")
        client.kill()
        client.log_text()
    else:
        client.gallery_frames("c2")
        # T flips the TEAM row; the text block must change with it.
        grab(os.path.join(EVID, "c2-team-p0.png"))
        key("T")
        grab(os.path.join(EVID, "c2-team-p1.png"))
        key("T")
        # Up/Down zoom: pull in one step and back.
        key("Up")
        grab(os.path.join(EVID, "c2-zoom-in.png"))
        key("Down")
        key("Escape")
        ok = client.wait_exit()
        out = client.log_text()
        models = sorted(MODEL_LINE.findall(out))
        if models != ["barracks", "ore_node", "rifleman"]:
            errs.append(f"the three pilot models did not load: {models}")
        skins = sorted(SKIN_LINE.findall(out))
        if skins != ["command_center", "guardian", "raider", "supply_depot", "turret", "worker"]:
            errs.append(f"the six fallback skins are wrong: {skins}")
        total, in_kind, in_text, outside, _ = pixel_diff(
            os.path.join(EVID, "c2-team-p0.png"), os.path.join(EVID, "c2-team-p1.png"))
        print(f"A-020 c2: T flips the TEAM row: {in_text} text-rect px differ ({total} total)")
        if in_text < 40:
            errs.append("the T key did not change the team row's pixels")
        if not ok:
            errs.append("Esc did not exit the gallery cleanly")
    results.append(report("c2", errs))

# c2b — the A-146 pin, live: exactly ONE diagnostic per corrupted kind.
if check("c2b", "the A-146 pin: a corrupted model kind prints exactly one diagnostic in the gallery run"):
    errs = []
    for pack, kind, tier in [
        (BAD_RIFLE, "rifleman", "PNG skin"),
        (BAD_BARRACKS, "barracks", "PNG skin"),
        (BAD_NODE, "ore_node", "placeholder silhouette"),
    ]:
        client = Client(["--gallery", "--pack", pack], f"c2b-{kind}.log")
        if client.win is None:
            errs.append(f"{kind}: the client window never appeared")
            client.kill()
            client.log_text()
            continue
        key("Escape")
        ok = client.wait_exit()
        out = client.log_text()
        count = out.count(f"entity '{kind}' —")
        if count != 1:
            errs.append(f"{kind}: the diagnostic appears {count} times, expected exactly 1")
        if f"rendering the {tier} tier" not in out:
            errs.append(f"{kind}: the one line does not name the {tier} tier")
        if not ok:
            errs.append(f"{kind}: Esc did not exit cleanly")
        print(f"A-020 c2b: {kind} diagnostic count = {count}, tier named: "
              f"{'yes' if f'rendering the {tier} tier' in out else 'NO'}")
    results.append(report("c2b", errs))

# c3 — THE PIXEL TEST per model kind: pilot (shown) vs corrupt (the model
# tier removed) at the same kind; the difference is the model's own pixels.
if check("c3", "the pixel test: each model kind's pixels land inside the expected screen rects, zero outside"):
    # The c2 frames are the pilot captures; the corrupt runs are fresh.
    corrupt_packs = {0: BAD_BARRACKS, 3: BAD_NODE, 5: BAD_RIFLE}
    pilot_frames = {i: os.path.join(EVID, f"c2-kind-{i:02d}.png") for i in MODEL_KINDS}
    corrupt_frames = {}
    for i, pack in corrupt_packs.items():
        client = Client(["--gallery", "--pack", pack], f"c3-{MODEL_KINDS[i]}.log")
        if client.win is None:
            print(f"A-020 c3: FAIL — {MODEL_KINDS[i]} corrupt-pack window never appeared")
            client.kill()
            client.log_text()
            results.append(False)
            corrupt_frames = {}
            break
        for j in range(i):
            key("Right")   # walk to kind i
        path = os.path.join(EVID, f"c3-{MODEL_KINDS[i]}-kind.png")
        grab(path)
        corrupt_frames[i] = path
        key("Escape")
        if not client.wait_exit():
            print(f"A-020 c3: FAIL — {MODEL_KINDS[i]} corrupt run did not exit")
            results.append(False)
            corrupt_frames = {}
            break
    if corrupt_frames:
        errs = []
        for i, kind in MODEL_KINDS.items():
            total, in_kind, in_text, outside, sample = pixel_diff(
                pilot_frames[i], corrupt_frames[i])
            minimum = KIND_MINIMUMS[kind]
            print(f"A-020 c3 {kind}: total={total} in_kind_rect={in_kind} "
                  f"in_text_rect={in_text} OUTSIDE={outside} sample={sample} "
                  f"(expected rects: kind={KIND_RECT} text={TEXT_RECT}, "
                  f"minimum in-kind={minimum})")
            if outside != 0:
                errs.append(f"{kind}: {outside} differing pixels outside the expected "
                            f"rects (first at {sample})")
            if in_kind < minimum:
                errs.append(f"{kind}: only {in_kind} in-rect pixels (minimum {minimum}) — "
                            "the model barely draws")
        results.append(report("c3", errs))

# c4 — the tier label follows the loaded pack.
if check("c4", "the tier label follows the pack: model (pilot) vs skin/placeholder (official) per kind"):
    errs = []
    for i, kind, official_tier in [(5, "rifleman", "skin"),
                                   (3, "ore_node", "placeholder")]:
        pilot_frame = os.path.join(EVID, f"c2-kind-{i:02d}.png")
        official_frame = os.path.join(EVID, f"c1-kind-{i:02d}.png")
        total, in_kind, in_text, outside, _ = pixel_diff(pilot_frame, official_frame)
        print(f"A-020 c4 {kind}: label-row diff vs the official pack: "
              f"{in_text} text-rect px (pilot tier=model, official tier={official_tier})")
        if in_text < 40:
            errs.append(f"{kind}: the tier label did not change between the packs")
        # The KIND rect must also differ — the model's pixels are not the
        # skin/placeholder's pixels.
        if in_kind < KIND_MINIMUMS[kind]:
            errs.append(f"{kind}: the pilot's model pixels did not survive the pack swap")
    results.append(report("c4", errs))

# c5 — the report frames: a spectated match on the pilot pack at the default
# camera; 4x crops of an ore node + a rifleman group; the gallery frames of
# the same kinds. Written into the report's frames folder (the brief's
# declared deliverable), plus the raw captures in the evidence dir.
if check("c5", "the report frames: spectate 4x crops + the gallery frames, into docs/pm/reports/a020-frames"):
    Path(FRAMES).mkdir(parents=True, exist_ok=True)
    log_path = os.path.join(EVID, "c5-spectate.log")
    with open(log_path, "w") as log_file, \
         subprocess.Popen([BIN, "--pack", PILOT, "--seed", "7"],
                          stdout=log_file, stderr=subprocess.STDOUT) as proc:
        before = {w.id for w in root.query_tree().children}
        win = None
        t0 = time.time()
        while time.time() - t0 < 30:
            new = [w for w in root.query_tree().children if w.id not in before]
            if new:
                win = new[-1]
                break
            if proc.poll() is not None:
                break
            time.sleep(0.2)
        errs = []
        if win is None:
            errs.append("the spectate window never appeared")
            proc.kill()
        else:
            win.set_input_focus(X.RevertToParent, X.CurrentTime)
            d.flush()
            time.sleep(6.0)
            # New Match -> spectate (Right) -> Start (Down x3) -> Enter.
            key("Return")
            key("Right")
            key("Down"); key("Down"); key("Down")
            key("Return")
            time.sleep(12.0)   # the opening: the ore nodes stand at their tiles
            grab(os.path.join(EVID, "c5-match-opening.png"))
            # Two minutes in, the AI has trained real rifleman groups (the
            # A-018 c4 timeline's own finding: riflemen take minutes to
            # exist; at 30s there are none to crop).
            time.sleep(110.0)
            grab(os.path.join(EVID, "c5-match-late.png"))
            close(win)
            try:
                proc.wait(timeout=30)
            except subprocess.TimeoutExpired:
                proc.kill()
    # The 4x crops from the LATE match frame (the default camera; twenty
    # more seconds of AI play than the opening means more riflemen on
    # screen). The ore node is static — its crystals sit at the same spot
    # every run — but the rifleman groups move with the match, so their
    # window is FOUND at run time: the densest bright-blue (P0 team) 140x140
    # window over the world view, scanned on the capture itself.
    crop_4x(os.path.join(EVID, "c5-match-late.png"), 190, 90, 100, 105,
            os.path.join(FRAMES, "ore-node-match-4x.png"))
    # The rifleman-group window is FOUND on the capture: the thin bright-blue
    # runs are the unit rings' and shoulder accents' signature — a big team
    # panel (a barracks roof, say) reads as one long run and is excluded, so
    # the densest window of short blue runs is where the units stand.
    mw, mh, mpx = read_png(os.path.join(EVID, "c5-match-late.png"))
    def blue(c):
        return c[2] > 120 and c[2] > c[0] + 40
    def short_blue_runs():
        tagged = set()
        for y in range(100, WIN_H):
            run = 0
            for x in range(0, WIN_W):
                if blue(mpx[y * mw + x]):
                    run += 1
                else:
                    if 2 <= run <= 20:
                        for dx in range(run):
                            tagged.add((x - 1 - dx, y))
                    run = 0
        return tagged
    tagged = short_blue_runs()
    best = (-1, 500, 180)
    for y0 in range(100, 460, 20):
        for x0 in range(160, 660, 20):
            n = sum(1 for (x, y) in tagged
                    if x0 <= x < x0 + 140 and y0 <= y < y0 + 100)
            if n > best[0]:
                best = (n, x0, y0)
    print(f"A-020 c5: densest rifleman window: {best[1], best[2]} "
          f"with {best[0]} thin blue runs (units' rings + accents)")
    if best[0] < 400:
        print("A-020 c5: FAIL")
        print(f"  - no rifleman group found on the late frame ({best[0]} thin runs)")
        results.append(False)
    else:
        crop_4x(os.path.join(EVID, "c5-match-late.png"), best[1], best[2], 140, 100,
                os.path.join(FRAMES, "rifleman-group-match-4x.png"))
    # The gallery frames of the same kinds (the stats text included).
    for i, name in [(3, "ore-node-gallery.png"), (5, "rifleman-gallery.png")]:
        w, h, px = read_png(os.path.join(EVID, f"c2-kind-{i:02d}.png"))
        windowed = [[px[y*w + x] for x in range(WIN_W)] for y in range(WIN_H)]
        rows = bytearray()
        for row in windowed:
            rows += b"\x00" + b"".join(struct.pack(">BBB", *p) for p in row)
        def chunk(tag, data):
            c = struct.pack(">I", len(data)) + tag + data
            return c + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)
        png = b"\x89PNG\r\n\x1a\n"
        png += chunk(b"IHDR", struct.pack(">IIBBBBB", WIN_W, WIN_H, 8, 2, 0, 0, 0))
        png += chunk(b"IDAT", zlib.compress(bytes(rows), 6))
        png += chunk(b"IEND", b"")
        Path(os.path.join(FRAMES, name)).write_bytes(png)
        print(f"A-020 frame: {os.path.join(FRAMES, name)} ({WIN_W}x{WIN_H})")
    # The crops must contain real pixels (not void) and the match frames must
    # be live renders.
    for frame in ["c5-match-opening.png", "c5-match-late.png"]:
        if not live_variance(os.path.join(EVID, frame)):
            errs.append(f"{frame} looks void (no pixel variance)")
    with open(log_path) as f:
        match_log = f.read()
    if "entity model 'ore_node'" not in match_log:
        errs.append("the spectate run did not load the ore node model")
    if "entity model 'rifleman'" not in match_log:
        errs.append("the spectate run did not load the rifleman model")
    results.append(report("c5", errs))

print()
if results and all(results):
    print(f"A-020: {sum(results)}/{len(results)} checks PASS")
    sys.exit(0)
print(f"A-020: {sum(results)}/{len(results)} checks PASS — FAILURES ABOVE")
sys.exit(1)
PYEOF
STATUS=$?
log ""
if [ "$STATUS" -eq 0 ]; then
    log "A-020 machine pass: all checks green (evidence in $EVIDENCE_DIR)"
else
    log "A-020 machine pass: FAILURES (evidence in $EVIDENCE_DIR)"
fi
exit $STATUS
