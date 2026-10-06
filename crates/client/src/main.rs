//! The playable Pandemonium binary (plan §11, ADR-0001): a winit window with a
//! wgpu 3D renderer (depth buffer, terrain mesh + display-only heightmap,
//! instanced placeholder boxes), perspective RTS camera controls, ground-plane
//! picking, and box selection. Input produces only `Command`s (FD-2) — the
//! simulation is owned by the engine's `MatchHost` and is unreachable for
//! mutation by any other means.
//!
//! Headless environments (CI): when no display is available the client runs a
//! headless smoke pass over the real content (load, advance, render through the
//! null renderer) and exits 0 with a printed finding — the windowed exit test
//! of M3 needs a display and is recorded as such (plan §13 honest declaration).

mod feedback;
mod input;
mod orders;
mod render;
mod report;
mod text;
mod ui;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use feedback::FeedbackState;
use input::{RightButton, RightRelease};
use pandemonium_ai::Controller;
use pandemonium_content::ContentBundle;
use pandemonium_engine::audio::{AudioSink, NullAudioSink};
use pandemonium_engine::mesh::terrain_mesh;
use pandemonium_engine::renderer::{Frame, HudState};
use pandemonium_engine::{
    alpha_controller, alpha_plan, FrameOutcome, Interpolator, MatchHost, MatchOutcome,
    NullRenderer, Renderer, RtsCamera,
};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, KindId, MatchSetup, PlayerId,
    PlayerSetup, PlayerView, Snapshot, TileFog, TilePos, Vec2Fx,
};
use render::WgpuRenderer;
use std::collections::BTreeMap;
use text::{TextAtlas, UiQuad};
use ui::{Button, ButtonAction};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, PhysicalKey};
use winit::window::{Window, WindowId};

/// The player the human controls.
const HUMAN: PlayerId = PlayerId(0);
/// The AI opponent's slot.
const AI: PlayerId = PlayerId(1);
/// How far the cursor can drift (in NDC) for a press-release to count as a
/// click rather than a drag.
const CLICK_SLOP: f32 = 0.01;
/// Heightmap units -> tiles of vertical displacement (display-only, ADR-0001).
const HEIGHT_SCALE: f32 = 0.02;
/// How many control groups the client tracks (plan §11.3: control groups).
const CONTROL_GROUP_COUNT: usize = 9;
/// Camera pan speed in tiles per second (keys and edge scrolling share it —
/// scaled by the real frame delta so pan speed does not depend on the
/// display's refresh rate; M9.1: the old fixed per-frame speed made a 144 Hz
/// monitor pan more than twice as fast as 60 Hz).
const PAN_TILES_PER_SECOND: f32 = 34.0;
/// Camera yaw rotation speed in radians per second (Generals-style Q/E
/// camera rotation). Held keys apply this rate, scaled by the frame
/// delta, so a full turn takes ~6 s — fast enough to re-orient without
/// inducing motion sickness.
const ROTATE_RAD_PER_SEC: f32 = pandemonium_engine::camera::DEFAULT_ROTATE_RAD_PER_SEC;
/// Pitch adjustment per wheel notch when Ctrl is held (Generals-style
/// Ctrl+wheel pitch control). ~12 notches cover the full pitch range.
const PITCH_PER_WHEEL_NOTCH: f32 = pandemonium_engine::camera::DEFAULT_PITCH_PER_WHEEL_NOTCH;
/// The frame-delta clamp for camera motion — a stall must not teleport the
/// view when the loop resumes.
const MAX_PAN_FRAME_SECONDS: f32 = 0.05;
/// The opening camera distance from the player's start anchor (M9.1: the
/// old map-center default at 58 tiles out rendered the starting force as
/// specks — "nothing appears on the screen").
const START_CAMERA_DISTANCE: f32 = 26.0;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let seed = seed_from_args(&args);
    let record_path = record_from_args(&args);
    let content = content_path();
    let bundle = ContentBundle::load_dir(content)
        .with_context(|| format!("loading content from {}", content.display()))?;

    let event_loop = match EventLoop::builder().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            // The M3 finding, printed honestly: the windowed experience needs a
            // display; CI and other headless machines get the smoke pass.
            println!(
                "pandemonium client — no display available ({error}); running the headless \
                 smoke pass (windowed mode needs X11/Wayland; see AI-Handoff §8)"
            );
            return headless_smoke(&bundle);
        }
    };
    let mut app = App::new(bundle, seed, record_path)?;
    app.frames_budget = frames_budget_arg(&args);
    event_loop
        .run_app(&mut app)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

/// Parses the verification affordance `--frames N`: exit cleanly after N
/// presented frames and print the windowed smoke summary (tick, state hash,
/// commands submitted, selection size). Presentation-layer tooling for the
/// DEBT-008 windowed verification — it changes no simulation behavior.
fn frames_budget_arg(args: &[String]) -> Option<u64> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--frames" {
            let value = args
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(180);
            return Some(value.max(1));
        }
    }
    None
}

/// Parses the reproduction affordance `--seed N` (M10.1, plan §6.5: a match
/// is seed + content + command log — the README's bug-report promise needs
/// the windowed player to be able to name the seed they played). Default 7
/// (the seed every existing pin and the M1 demo use); a missing or malformed
/// value falls back to the default, mirroring `--frames`'s parser.
fn seed_from_args(args: &[String]) -> u64 {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--seed" {
            return args
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(7);
        }
    }
    7
}

/// Parses the recording affordance `--record <path>` (M10.1, plan §6.5):
/// when set, the windowed run writes its replay at match end / clean exit —
/// a file `tools replay-verify` accepts (A2's evidence channel for the
/// human playtest). A missing value disables recording.
fn record_from_args(args: &[String]) -> Option<PathBuf> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--record" {
            return args.next().cloned().map(PathBuf::from);
        }
    }
    None
}

/// The repository's content directory, resolved from the crate manifest so the
/// binary works from any working directory.
fn content_path() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"))
}

/// The windowed application state.
struct App {
    /// The loaded content bundle (M8: retained for restart — fresh controllers
    /// are re-derived from the bundle + seed on each restart).
    bundle: ContentBundle,
    /// The match setup (M8: retained for restart — the same seed reproduces
    /// the same match bit-for-bit, A15).
    setup: MatchSetup,
    host: MatchHost,
    camera: RtsCamera,
    renderer: Option<WgpuRenderer>,
    window: Option<Arc<Window>>,
    selection: Vec<EntityId>,
    /// M8 (plan §11.3): control groups 1-9. Ctrl+digit assigns the current
    /// selection to a group; digit alone recalls it.
    control_groups: Vec<Vec<EntityId>>,
    drag_start: Option<(f64, f64)>,
    drag_current: Option<(f64, f64)>,
    keys: BTreeSet<Key<&'static str>>,
    /// M8: the current keyboard modifiers (Ctrl for control-group assignment).
    modifiers: ModifiersState,
    /// M10.2 (PLAN §1.2): the right button's disambiguation state — a
    /// press-and-release under [`input::RIGHT_DRAG_COMMAND_MAX_PX`] is the
    /// command click; travel beyond it is the Generals-style grab-and-drag
    /// map scroll (and the release then orders nothing).
    right_button: RightButton,
    /// M10.2 (PLAN §1.2): the ground point grabbed at the right-button
    /// press — the anchor the drag keeps under the cursor while scrolling.
    right_anchor: Option<glam::Vec3>,
    /// M10.2: whether attack-move was armed when the right button went
    /// down (the minimap right-click honors it as an attack-move order; a
    /// world right-click disarms and issues the context order).
    right_press_armed: bool,
    /// M10.2 (PLAN §1.2): the middle button's last cursor position while
    /// held — horizontal travel rotates the camera (Generals-style
    /// middle-drag rotate; it panned before this pass).
    middle_last: Option<(f64, f64)>,
    /// M10.2 (PLAN §1.3): when the last left *click* (not drag) landed —
    /// the double-click window's clock for same-kind select-all.
    last_left_click: Option<Instant>,
    /// M10.2 (PLAN §1.3): the last control-group digit press and when it
    /// landed — a double tap recalls the group and centers the camera on it.
    last_digit_press: Option<(usize, Instant)>,
    /// M10.2 (PLAN §1.2): whether edge scrolling is on (the settings toggle
    /// itself is Phase 3; the flag is wired now so the toggle only flips a
    /// bool then). The band width and depth scaling live in `input.rs`.
    edge_scroll_enabled: bool,
    /// M9.1: whether the window has keyboard focus (edge scrolling runs
    /// focused only, so an unfocused game never steals the desktop).
    focused: bool,
    /// M9.1 (plan §11.3): 'A' arms attack-move; the next left-click places
    /// it. Firing on key-down fought the key's camera-pan role and spammed
    /// orders through key repeats (the DEBT-008 human pass: "the control
    /// is bad").
    attack_move_armed: bool,
    /// M9.1: the last submitted order (its seq and the ground point it
    /// targeted) — rejection feedback pings there.
    last_order: Option<(u32, Vec2Fx)>,
    /// M9.1: the human start anchor (the opening camera focus and restart's
    /// re-focus), from the map's declared starts.
    start_anchor: (f32, f32),
    /// M9.1: the content's worker kind id (right-click gather resolution).
    worker_kind: Option<KindId>,
    /// M9.1: the content's gatherable node kind id (right-click gather
    /// resolution).
    node_kind: Option<KindId>,
    last_frame: Instant,
    command_seq: u32,
    /// Display names of the loaded resources, indexed by ResourceId (the
    /// HUD's labels — data, not hardcoded).
    resource_names: Vec<String>,
    /// Whether the §11.6 debug overlay is visible (F3).
    debug_overlay: bool,
    /// Verification affordance: exit after this many presented frames.
    frames_budget: Option<u64>,
    /// How many frames have been presented (the `--frames` counter).
    frames_presented: u64,
    /// How many commands the client has submitted this run (the windowed
    /// smoke summary's evidence that input reached the simulation).
    commands_submitted: u32,
    /// M9 (plan §11.5): the audio sink fed by the step's events — the null
    /// implementation (cue-counting; a mixer swaps in post-Alpha behind the
    /// same trait).
    audio: NullAudioSink,
    /// M9 (plan §11.2/§11.5, the feel pass): event-driven feedback — hit
    /// flashes, command acknowledgment pings, and the counters the smoke
    /// summary reports.
    feedback: FeedbackState,
    /// The client's own interpolator over the *fog-filtered* view: the human
    /// sees what the human's player can see (FD-8 from the rendering side).
    /// The host's own interpolator keeps blending full snapshots for anyone
    /// who needs them; everything drawn here comes from this one.
    view_interp: Interpolator,
    /// The sim tick the fog texture was last refreshed at (fog updates once
    /// per sim step, not once per presented frame). `u64::MAX` forces the
    /// first refresh.
    fog_tick: u64,
    /// The last fog alpha bytes (row-major, one per tile) — compared against
    /// the incoming view so a new texture upload only happens on change.
    fog_bytes: Vec<u8>,
    /// Every visible entity's last-seen world position, kind, and owner —
    /// the data a death fade needs at the moment a `Died` event arrives
    /// (the event carries only the id).
    last_seen: BTreeMap<EntityId, (glam::Vec3, KindId, PlayerId)>,
    /// The player's own view as of this frame — the command card's data
    /// source (queues, own entities for requirement checks, the ledger).
    latest_view: Option<PlayerView>,
    /// Active structure placement (§11.3's build placement mode): the
    /// worker that will build, the structure, and the tile the ghost
    /// currently hovers (None until the cursor first hits the ground).
    placement: Option<PlacementState>,
    /// The bottom bar's clickable regions, rebuilt every frame by the UI
    /// pass and hit-tested before world clicks.
    ui_buttons: Vec<Button>,
    /// M10.1 (plan §6.5): `--record <path>` — when set, the windowed run's
    /// replay is written at match end / clean exit (a file `tools
    /// replay-verify` accepts — A2's evidence channel for the playtest).
    record_path: Option<PathBuf>,
    /// M10.1: whether the current match segment's record was already
    /// written at its end (the exit path writes only when it never was —
    /// restart resets this with the segment).
    record_written: bool,
    /// M10.1 (plan §6.5/§11.6): the checkpoint trail feeding `--record` and
    /// the F8 dump — the tick-0 hash plus every periodic checkpoint the
    /// host reported per frame (the same assembly the tools' AI-match
    /// recorder uses).
    checkpoints: Vec<(u32, u64)>,
}

/// Active structure placement (plan §11.3's build placement mode with a
/// legality preview).
struct PlacementState {
    /// The worker that will receive the Build command.
    worker: EntityId,
    /// The structure kind to place.
    structure: KindId,
    /// The tile the ghost currently hovers over.
    ghost_tile: Option<(i32, i32)>,
}

impl App {
    fn new(bundle: ContentBundle, seed: u64, record_path: Option<PathBuf>) -> anyhow::Result<Self> {
        let setup = MatchSetup {
            seed,
            players: vec![
                PlayerSetup {
                    player: PlayerId(0),
                    controller: ControllerKind::Human,
                },
                PlayerSetup {
                    player: PlayerId(1),
                    controller: ControllerKind::Ai,
                },
            ],
        };
        let resource_names: Vec<String> = bundle
            .rules
            .resources
            .iter()
            .map(|resource| resource.display_name.clone())
            .collect();
        // M9.1: open on the player's base, not the map center — the start
        // anchor with its starting force fills the frame at the closer
        // distance, so the player sees their units on frame one.
        let start_anchor = bundle
            .map
            .starts
            .iter()
            .find(|start| start.player == 0)
            .map(|start| (start.x as f32 + 0.5, start.y as f32 + 0.5))
            .unwrap_or((
                bundle.map.width as f32 / 2.0,
                bundle.map.height as f32 / 2.0,
            ));
        let mut camera = RtsCamera::new(bundle.map.width, bundle.map.height, 16.0 / 9.0);
        camera.focus(start_anchor.0, start_anchor.1, START_CAMERA_DISTANCE);
        // M9.1: the right-click context resolver needs the content's worker
        // and node kind ids — the engine's alpha-plan resolution (capability
        // shaped, never name-matched) already derives exactly those.
        let world = bundle.world();
        let plan = alpha_plan(&bundle, &world, HUMAN, setup.seed);
        let worker_kind = Some(plan.worker);
        let node_kind = plan.node;
        let host = Self::build_host(&bundle, setup.clone());
        // M10.1: the checkpoint trail opens at the fresh match's tick-0
        // hash (the same first entry the tools' recorder pushes).
        let checkpoints = vec![(0, host.state_hash())];
        Ok(Self {
            bundle,
            setup,
            host,
            camera,
            renderer: None,
            window: None,
            selection: Vec::new(),
            control_groups: vec![Vec::new(); CONTROL_GROUP_COUNT],
            drag_start: None,
            drag_current: None,
            keys: BTreeSet::new(),
            modifiers: ModifiersState::empty(),
            right_button: RightButton::new(),
            right_anchor: None,
            right_press_armed: false,
            middle_last: None,
            last_left_click: None,
            last_digit_press: None,
            edge_scroll_enabled: true,
            focused: true,
            attack_move_armed: false,
            last_order: None,
            start_anchor,
            worker_kind,
            node_kind,
            last_frame: Instant::now(),
            command_seq: 0,
            resource_names,
            debug_overlay: false,
            frames_budget: None,
            frames_presented: 0,
            commands_submitted: 0,
            audio: NullAudioSink::new(),
            feedback: FeedbackState::default(),
            view_interp: Interpolator::new(),
            fog_tick: u64::MAX,
            fog_bytes: Vec::new(),
            last_seen: BTreeMap::new(),
            latest_view: None,
            placement: None,
            ui_buttons: Vec::new(),
            record_path,
            record_written: false,
            checkpoints,
        })
    }

    /// Builds a fresh `MatchHost` with a fresh AI controller for the opponent
    /// slot (M8). Used at construction and on restart — the controller's RNG
    /// seed derives from the match seed (plan §9.6: seeded, never its own
    /// entropy), so a same-seed restart reproduces the same match.
    fn build_host(bundle: &ContentBundle, setup: MatchSetup) -> MatchHost {
        let world = bundle.world();
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
            AI,
            Box::new(alpha_controller(bundle, &world, AI, setup.seed)),
        )];
        MatchHost::with_controllers(&world, setup, controllers)
    }

    /// Restarts the match: drops the current host and builds a fresh one from
    /// the same bundle + setup (M8, plan §9.7, A15 — "new Sim from the same
    /// setup with a new seed, with no leaked state"). The fresh controller
    /// re-derives its RNG seed from the match seed, so a same-seed restart
    /// reproduces the same match bit-for-bit. The camera, selection, control
    /// groups, and counters all reset.
    fn restart(&mut self) {
        let setup = self.setup.clone();
        self.host = Self::build_host(&self.bundle, setup);
        self.selection = Vec::new();
        self.control_groups = vec![Vec::new(); CONTROL_GROUP_COUNT];
        self.command_seq = 0;
        self.commands_submitted = 0;
        // The fog-filtered view restarts with the match: a fresh interpolator
        // and a forced fog refresh (the new match's fog is mostly hidden).
        self.view_interp = Interpolator::new();
        self.fog_tick = u64::MAX;
        self.fog_bytes.clear();
        self.last_seen.clear();
        self.latest_view = None;
        self.placement = None;
        self.ui_buttons.clear();
        // M10.2: the input-layer additions reset with the match — the right
        // button's gesture state, the middle-drag rotate anchor, and the
        // double-click clock.
        self.right_button = RightButton::new();
        self.right_anchor = None;
        self.right_press_armed = false;
        self.middle_last = None;
        self.last_left_click = None;
        self.last_digit_press = None;
        // M10.1: the recording state restarts with the match — the new
        // segment gets its own tick-0 checkpoint and its own end-of-match
        // record write.
        self.checkpoints = vec![(0, self.host.state_hash())];
        self.record_written = false;
        // M9.1: the camera re-frames on the base and the armed order clears
        // — a fresh match starts from the same readable opening view.
        self.camera.focus(
            self.start_anchor.0,
            self.start_anchor.1,
            START_CAMERA_DISTANCE,
        );
        self.attack_move_armed = false;
        self.last_order = None;
        // M9: the feel pass's state resets with the match — a fresh match
        // starts with clean feedback (no stale flashes or pings).
        self.audio = NullAudioSink::new();
        self.feedback = FeedbackState::default();
    }

    /// Converts window pixel coordinates to NDC (Y up).
    fn to_ndc(window: &Window, x: f64, y: f64) -> (f32, f32) {
        let size = window.inner_size();
        (
            (x / size.width.max(1) as f64) as f32 * 2.0 - 1.0,
            1.0 - (y / size.height.max(1) as f64) as f32 * 2.0,
        )
    }

    /// The fog-filtered interpolated snapshot for this display frame: the
    /// same blend point the host's full-snapshot path uses, over the entity
    /// set the human's player may see. Every drawn pixel and every screen
    /// query (selection, context orders) reads through this.
    fn view_snapshot(&self) -> pandemonium_engine::RenderSnapshot {
        self.view_interp.render(self.host.alpha())
    }

    /// M10.1 (plan §6.5): folds one frame outcome's periodic checkpoints
    /// into the trail `--record` and F8 read (the same accumulation the
    /// tools' AI-match recorder performs).
    fn collect_checkpoints(&mut self, outcome: &FrameOutcome) {
        for &(tick, hash) in &outcome.hashes {
            self.checkpoints.push((tick, hash));
        }
    }

    /// M10.1 (plan §6.5): writes the current match segment's replay record
    /// to the `--record` path — a file `tools replay-verify` accepts, built
    /// exactly like the tools' recorder (content hash, map id, the host's
    /// command log, the checkpoint trail, the forced final hash).
    fn write_recording(&mut self) {
        let Some(path) = self.record_path.clone() else {
            return;
        };
        let world = self.bundle.world();
        let replay = report::replay_record(
            &world,
            &self.setup,
            self.host.log(),
            &self.checkpoints,
            self.host.tick(),
            self.host.state_hash(),
        );
        let bytes = replay.encode();
        match std::fs::write(&path, &bytes) {
            Ok(()) => {
                println!(
                    "pandemonium client — replay recorded: {} ({} bytes, {} checkpoints, final \
                     hash {:#018x})",
                    path.display(),
                    bytes.len(),
                    replay.checkpoints.len(),
                    replay.final_hash
                );
                self.record_written = true;
            }
            Err(error) => {
                // Not recorded: the exit path may try again once the run ends
                // or the loop exits (a transient I/O failure should not lose
                // the recording).
                println!("pandemonium client — replay record failed: {error}");
            }
        }
    }

    /// M10.1 (plan §11.6): the deterministic F8 bug-report dump — the replay
    /// record of the match so far (`pandemonium-report-<seed>-tick<tick>.pdrp`)
    /// plus the sidecar info file (`…-info.txt`), written beside `--record`'s
    /// path or in the working directory, both filenames printed so the tester
    /// can attach them to the report. Works while paused (the dump reads the
    /// current state); deterministic content only, so two dumps at the same
    /// tick produce identical bytes.
    fn dump_bug_report(&mut self) {
        let world = self.bundle.world();
        let tick = self.host.tick();
        let replay = report::replay_record(
            &world,
            &self.setup,
            self.host.log(),
            &self.checkpoints,
            tick,
            self.host.state_hash(),
        );
        let sidecar = report::sidecar_text(&report::SidecarInfo {
            seed: self.setup.seed,
            tick,
            content_hash: replay.content_hash,
            map_id: replay.map_id,
            player_slot: HUMAN.0,
            frame_count: self.frames_presented,
            selection_size: self.selection.len(),
            controls_line: report::controls_line(
                self.attack_move_armed,
                self.feedback.refusal_notice.is_some(),
            ),
        });
        // Beside --record's path when given, the working directory otherwise.
        let dir = match &self.record_path {
            Some(path) => path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(Path::to_path_buf)
                .unwrap_or_default(),
            None => PathBuf::from("."),
        };
        let stem = report::report_stem(self.setup.seed, tick);
        match report::write_files(&dir, &stem, &replay, &sidecar) {
            Ok((replay_path, info_path)) => {
                println!("pandemonium bug report: {}", replay_path.display());
                println!("pandemonium bug report: {}", info_path.display());
                // The on-screen cue (plan §11.6): a brief ping where the
                // tester is looking, so the dump's success is visible.
                let ndc = self.cursor_ndc();
                let ground = self
                    .camera
                    .ground_point(glam::Vec2::new(ndc.0, ndc.1))
                    .map(|point| MatchHost::world_to_logical(point.x, point.z))
                    .unwrap_or_else(|| {
                        Vec2Fx::from_ints(self.start_anchor.0 as i32, self.start_anchor.1 as i32)
                    });
                self.feedback.ping(ground, self.frames_presented);
            }
            Err(error) => println!("pandemonium bug report: write failed ({error})"),
        }
    }

    /// The cursor's current NDC position (M8: used by the AttackMove hotkey,
    /// which fires on key-down without a fresh cursor event). Falls back to
    /// the screen center when the cursor hasn't moved yet.
    fn cursor_ndc(&self) -> (f32, f32) {
        let Some(window) = &self.window else {
            return (0.0, 0.0);
        };
        let (x, y) = self.drag_current.unwrap_or_else(|| {
            let size = window.inner_size();
            (size.width as f64 / 2.0, size.height as f64 / 2.0)
        });
        Self::to_ndc(window, x, y)
    }

    /// The pick under a single left click: the nearest own entity to the
    /// cursor within the pick radius (`None` on empty ground). M10.2
    /// (PLAN §1.1): what the *click then does* is the release handler's
    /// decision — select, shift-toggle, or double-click expand — this only
    /// answers "what is under the cursor".
    fn pick_own_entity(&self, ndc: (f32, f32)) -> Option<EntityId> {
        let snapshot = self.view_snapshot();
        let mut best: Option<(f32, EntityId)> = None;
        for entity in &snapshot.entities {
            if entity.owner != HUMAN {
                continue; // Selection picks own units (placeholder rule, M6 adds enemies).
            }
            let projected = self.camera.project(entity.pos);
            let distance = ((projected.x - ndc.0).powi(2) + (projected.y - ndc.1).powi(2)).sqrt();
            if distance < 0.05 && best.is_none_or(|(best_distance, _)| distance < best_distance) {
                best = Some((distance, entity.id));
            }
        }
        best.map(|(_, id)| id)
    }

    /// The ids inside a drag box over own entities (M10.2: the pure pick —
    /// shift-union vs replace is the release handler's decision).
    fn box_pick(&self, a: (f32, f32), b: (f32, f32)) -> Vec<EntityId> {
        let snapshot = self.view_snapshot();
        let own: Vec<pandemonium_engine::RenderEntity> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.owner == HUMAN)
            .copied()
            .collect();
        self.camera
            .box_select(&own, glam::Vec2::new(a.0, a.1), glam::Vec2::new(b.0, b.1))
    }

    /// The one place orders enter the host: stamps the seq, pings the
    /// acknowledgment, and remembers the order for rejection feedback
    /// (M9.1 — the last two are the "the control works" cues).
    fn submit_order(&mut self, kind: CommandKind, ping_at: Option<Vec2Fx>) {
        self.command_seq += 1;
        self.commands_submitted += 1;
        if let Some(at) = ping_at {
            self.feedback.ping(at, self.frames_presented);
            self.last_order = Some((self.command_seq, at));
        }
        self.host
            .submit(Command::new(HUMAN, 0, self.command_seq, kind));
    }

    /// M9.1 (plan §11.3): the right-click context command — what is under
    /// the cursor decides the order. An enemy issues Attack (the player's
    /// only way to order an attack — previously impossible), a resource
    /// node issues Gather for the selection's workers, open ground issues
    /// Move. The gate stays the authority; refusals surface as feedback.
    fn issue_context_order(&mut self, ndc: (f32, f32)) {
        if self.selection.is_empty() {
            return;
        }
        // A single selected producer sets its rally with a right-click (the
        // §11.4 rally point; buildings cannot move, so the ground context
        // order means "deliver here", not "go there").
        if self.selection.len() == 1 {
            let selected = self.selection[0];
            let is_producer = self.latest_view.as_ref().is_some_and(|view| {
                view.production
                    .iter()
                    .any(|queue| queue.producer == selected)
            });
            if is_producer {
                let Some(point) = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1)) else {
                    return;
                };
                let target = MatchHost::world_to_logical(point.x, point.z);
                self.submit_order(
                    CommandKind::SetRally {
                        producer: selected,
                        target,
                    },
                    Some(target),
                );
                return;
            }
        }
        let snapshot = self.view_snapshot();
        let Some(order) = orders::resolve_context_order(
            &self.camera,
            &snapshot,
            glam::Vec2::new(ndc.0, ndc.1),
            HUMAN,
            self.node_kind,
            self.worker_kind,
            &self.selection,
        ) else {
            return; // cursor above the horizon — nothing to order
        };
        match order {
            orders::ContextOrder::Attack { target, at } => self.submit_order(
                CommandKind::Attack {
                    units: self.selection.clone(),
                    target,
                },
                Some(at),
            ),
            orders::ContextOrder::Gather { node, at, units } => {
                self.submit_order(CommandKind::Gather { units, node }, Some(at));
            }
            orders::ContextOrder::Move { target } => self.submit_order(
                CommandKind::Move {
                    units: self.selection.clone(),
                    target,
                },
                Some(target),
            ),
        }
    }

    /// M8 (plan §11.3): Stop hotkey — clears the selection's order queues and
    /// any auto-acquired combat targets (the gate's Stop command).
    fn issue_stop(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.submit_order(
            CommandKind::Stop {
                units: self.selection.clone(),
            },
            None,
        );
    }

    /// M8 (plan §11.3): AttackMove — the selection moves to the picked
    /// ground point, engaging enemies encountered en route (combat semantics
    /// from M6). M9.1: issued only from the armed-'A' left-click, never from
    /// the key-down itself.
    fn issue_attack_move(&mut self, ndc: (f32, f32)) {
        if self.selection.is_empty() {
            return;
        }
        let Some(point) = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1)) else {
            return;
        };
        let target = MatchHost::world_to_logical(point.x, point.z);
        self.submit_order(
            CommandKind::AttackMove {
                units: self.selection.clone(),
                target,
            },
            Some(target),
        );
    }

    /// M8 (plan §11.3): assign the current selection to control group `index`
    /// (0-8 mapping to display 1-9).
    fn assign_control_group(&mut self, index: usize) {
        if index < self.control_groups.len() {
            self.control_groups[index] = self.selection.clone();
        }
    }

    /// Runs a bottom-bar button press (the input pass hit-tested it). Every
    /// action funnels through [`App::submit_order`] — the same gate, the
    /// same refusal cues (FD-2).
    fn press_button(&mut self, action: ButtonAction) {
        match action {
            ButtonAction::Train { producer, unit } => {
                self.submit_order(CommandKind::Train { producer, unit }, None);
            }
            ButtonAction::PlaceStructure { worker, structure } => {
                self.placement = Some(PlacementState {
                    worker,
                    structure,
                    ghost_tile: None,
                });
            }
            ButtonAction::CancelQueueItem { producer, index } => {
                self.submit_order(CommandKind::CancelQueueItem { producer, index }, None);
            }
        }
    }

    /// Places the pending structure at the ghost tile (the click half of
    /// placement mode). The gate is the authority; a wrong guess surfaces
    /// as the usual refusal cue.
    fn place_pending_structure(&mut self) {
        let Some(placement) = &self.placement else {
            return;
        };
        let Some((tile_x, tile_y)) = placement.ghost_tile else {
            return;
        };
        let (worker, structure) = (placement.worker, placement.structure);
        let at = TilePos {
            x: tile_x,
            y: tile_y,
        };
        self.submit_order(
            CommandKind::Build {
                worker,
                structure,
                at,
            },
            Some(Vec2Fx::from_ints(at.x, at.y)),
        );
        self.placement = None;
    }

    /// M8 (plan §11.3): recall control group `index` (0-8). Selection becomes
    /// the group's members that are still alive (the snapshot filters the
    /// dead — ids are never reused, so a stale group reference is harmless).
    fn recall_control_group(&mut self, index: usize) {
        if index >= self.control_groups.len() {
            return;
        }
        let live: std::collections::BTreeSet<EntityId> = self
            .view_snapshot()
            .entities
            .iter()
            .map(|entity| entity.id)
            .collect();
        self.selection = self.control_groups[index]
            .iter()
            .copied()
            .filter(|id| live.contains(id))
            .collect();
    }
}

/// Tracks a WASD key's state (camera panning).
fn set_key(keys: &mut BTreeSet<Key<&'static str>>, character: &'static str, pressed: bool) {
    let key = Key::Character(character);
    if pressed {
        keys.insert(key);
    } else {
        keys.remove(&key);
    }
}

/// M8: maps a DigitN KeyCode to its 1-based group index (1..=9). Used by the
/// control-group hotkeys.
fn digit_to_group_index(code: winit::keyboard::KeyCode) -> usize {
    match code {
        winit::keyboard::KeyCode::Digit1 => 1,
        winit::keyboard::KeyCode::Digit2 => 2,
        winit::keyboard::KeyCode::Digit3 => 3,
        winit::keyboard::KeyCode::Digit4 => 4,
        winit::keyboard::KeyCode::Digit5 => 5,
        winit::keyboard::KeyCode::Digit6 => 6,
        winit::keyboard::KeyCode::Digit7 => 7,
        winit::keyboard::KeyCode::Digit8 => 8,
        winit::keyboard::KeyCode::Digit9 => 9,
        _ => 0,
    }
}

/// Derives one silhouette per kind from the content's capability
/// composition — the same capability-shaped law the engine's plan resolution
/// follows (never a name match). Ore nodes (Resource) read as amber
/// clusters, turrets (Attack + Footprint, no Move) as slabs with barrels,
/// structures (Footprint) as footprint-sized buildings, movers as body+head
/// units whose size varies deterministically by kind so infantry kinds read
/// apart without any content edits.
fn kind_shapes(bundle: &ContentBundle) -> Vec<render::KindShape> {
    bundle
        .entities
        .iter()
        .enumerate()
        .map(|(index, def)| {
            let has = |name: &str| def.capability(name).is_some();
            let kind = if has("Resource") {
                render::ShapeKind::Node
            } else if has("Attack") && def.footprint().is_some() && !has("Move") {
                render::ShapeKind::Turret
            } else if let Some((w, h)) = def.footprint() {
                render::ShapeKind::Structure {
                    w: w as f32,
                    h: h as f32,
                }
            } else {
                // Deterministic per-kind size variation within infantry
                // proportions (kind index, not a name or an RNG).
                let wide = 0.36 + ((index + 1) % 3) as f32 * 0.05;
                let tall = 0.52 + (index % 3) as f32 * 0.07;
                render::ShapeKind::Unit {
                    body: [wide, tall, wide],
                    head: [wide * 0.55, tall * 0.34, wide * 0.55],
                }
            };
            // Structures and nodes tint themselves in build_entity_instances;
            // units get a mild per-kind cool/warm shift so kinds read apart.
            let shift = ((index % 3) as f32 - 1.0) * 0.05;
            let tint = [1.0 + shift, 1.0, 1.0 - shift];
            render::KindShape { kind, tint }
        })
        .collect()
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let window = Arc::new(
            event_loop
                .create_window(Window::default_attributes().with_title("Pandemonium"))
                .expect("creating the window"),
        );
        self.camera.set_aspect(
            window.inner_size().width as f32 / window.inner_size().height.max(1) as f32,
        );
        match WgpuRenderer::new(
            window.clone(),
            &terrain_mesh_of(),
            kind_shapes(&self.bundle),
        ) {
            Ok(renderer) => {
                self.renderer = Some(renderer);
                self.window = Some(window);
            }
            Err(error) => {
                eprintln!("pandemonium client — GPU initialization failed: {error:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(size.width, size.height);
                    self.camera
                        .set_aspect(size.width as f32 / size.height.max(1) as f32);
                }
            }
            WindowEvent::RedrawRequested => {
                self.draw();
                // M9.1: the presented-frame counter is the feedback clock
                // (flashes and pings expire by it) — it advances on every
                // presented frame, not only in the `--frames` verification
                // mode. In normal play it was pinned at zero, so M9's cues
                // never expired and accumulated forever.
                self.frames_presented += 1;
                // The windowed smoke budget (DEBT-008 verification): exit after
                // N presented frames, with the evidence summary.
                if let Some(budget) = self.frames_budget {
                    if self.frames_presented >= budget {
                        self.windowed_summary();
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let _ = self.drag_current.insert((position.x, position.y));
                // M10.2 (PLAN §1.2): right-drag map scrolling — once the
                // right button's gesture crosses the command threshold, the
                // ground point grabbed at the press follows the cursor
                // ("grab the ground and pull", Generals ZH style). The same
                // anchor math the old middle-drag pan used; the release of
                // a scrolled gesture issues no command.
                if self.right_button.moved((position.x, position.y)) {
                    // The crossing event: grab the ground under the press
                    // if it was not grabbed yet (a press with no ground
                    // under it grabs at the first crossing instead).
                    if self.right_anchor.is_none() {
                        let ndc = Self::to_ndc(
                            self.window.as_ref().expect("window exists in CursorMoved"),
                            position.x,
                            position.y,
                        );
                        self.right_anchor = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1));
                    }
                }
                if self.right_button.is_dragging() {
                    if let Some(anchor) = self.right_anchor {
                        let Some(window) = &self.window else {
                            return;
                        };
                        let ndc = Self::to_ndc(window, position.x, position.y);
                        if let Some(current) =
                            self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1))
                        {
                            self.camera.pan_world(anchor - current);
                        }
                    }
                }
                // M10.2 (PLAN §1.2): middle-drag rotates the camera
                // (Generals does this — it panned before). Horizontal
                // travel orbits the view; vertical travel is inert.
                if let Some((last_x, _)) = self.middle_last {
                    let dx = (position.x - last_x) as f32;
                    self.camera.rotate(input::MIDDLE_DRAG_RAD_PER_PX * dx);
                    self.middle_last = Some((position.x, position.y));
                }
                // Placement mode tracks the cursor's ground tile for the
                // ghost (plan §11.3's placement preview).
                if self.placement.is_some() {
                    let Some(window) = &self.window else {
                        return;
                    };
                    let ndc = Self::to_ndc(window, position.x, position.y);
                    if let Some(point) = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1)) {
                        if let Some(placement) = &mut self.placement {
                            placement.ghost_tile =
                                Some((point.x.floor() as i32, point.z.floor() as i32));
                        }
                    }
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(window) = &self.window else { return };
                let cursor = self.drag_current.unwrap_or((0.0, 0.0));
                let ndc = Self::to_ndc(window, cursor.0, cursor.1);
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        // Bottom-bar buttons consume the press first (the
                        // command card is UI, not world).
                        let (px, py) = cursor;
                        if let Some(button) = self
                            .ui_buttons
                            .iter()
                            .find(|button| button.contains(px as f32, py as f32))
                            .copied()
                        {
                            if button.enabled {
                                self.press_button(button.action);
                            }
                            return;
                        }
                        // A minimap click moves the camera (plan §11.4's
                        // click-to-move-camera).
                        let minimap_hit = self
                            .renderer
                            .as_ref()
                            .and_then(|renderer| renderer.minimap_hit(px as f32, py as f32));
                        if let Some((world_x, world_z)) = minimap_hit {
                            let distance = self.camera.distance();
                            self.camera.focus(world_x, world_z, distance);
                            return;
                        }
                        if self.placement.is_some() {
                            // Placement mode: the click places (or, on an
                            // unhovered ghost, waits for a ground tile).
                            self.place_pending_structure();
                            return;
                        }
                        if self.attack_move_armed {
                            // M9.1: the armed attack-move fires at the click,
                            // consumes the press, and disarms — the press
                            // never starts a selection drag.
                            self.attack_move_armed = false;
                            self.issue_attack_move(ndc);
                        } else {
                            self.drag_start = Some(cursor);
                        }
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        if let Some(start) = self.drag_start.take() {
                            let start_ndc = Self::to_ndc(window, start.0, start.1);
                            let dragged = (start_ndc.0 - ndc.0).abs() > CLICK_SLOP
                                || (start_ndc.1 - ndc.1).abs() > CLICK_SLOP;
                            let shift = self.modifiers.shift_key();
                            if dragged {
                                // Box select (M10.2 PLAN §1.3: shift+box
                                // ADDS to the selection instead of
                                // replacing it).
                                let picked = self.box_pick(start_ndc, ndc);
                                self.selection = if shift {
                                    input::union_selection(&self.selection, &picked)
                                } else {
                                    picked
                                };
                            } else {
                                // A click. M10.2 (PLAN §1.3): a double click
                                // (two clicks inside the window) selects
                                // all visible units of the same kind.
                                let now = Instant::now();
                                let double = self.last_left_click.is_some_and(|then| {
                                    input::is_double_click(
                                        now.duration_since(then).as_millis() as u64
                                    )
                                });
                                self.last_left_click = Some(now);
                                if let Some(picked) = self.pick_own_entity(ndc) {
                                    if double {
                                        let same_kind = input::same_kind_selection(
                                            &self.view_snapshot().entities,
                                            picked,
                                        )
                                        .unwrap_or_default();
                                        self.selection = if shift {
                                            input::union_selection(&self.selection, &same_kind)
                                        } else {
                                            same_kind
                                        };
                                    } else if shift {
                                        // Shift+click toggles membership.
                                        self.selection =
                                            input::shift_click_selection(&self.selection, picked);
                                    } else {
                                        self.selection = vec![picked];
                                    }
                                }
                                // else: a click on empty ground. PLAN §1.1:
                                // "Click on empty ground with a selection
                                // and nothing else = nothing happens" —
                                // never an order (left never orders), and
                                // not even a deselect; Escape clears the
                                // selection.
                            }
                        }
                    }
                    // M10.2 (PLAN §1.2): the right button is the command
                    // button *and*, held and dragged past the threshold,
                    // the map-scroll grab (Generals ZH style). The press
                    // only records; the RELEASE decides — press-to-release
                    // travel under `input::RIGHT_DRAG_COMMAND_MAX_PX` is
                    // the context command, beyond it the drag has already
                    // scrolled and the release orders nothing.
                    (MouseButton::Right, ElementState::Pressed) => {
                        if self.placement.take().is_some() {
                            // Right-click cancels placement and orders
                            // nothing: the whole gesture is consumed (the
                            // machine stays Idle, so the release is a
                            // no-op too).
                            self.attack_move_armed = false;
                            return;
                        }
                        // Capture whether attack-move was armed at the
                        // press: the minimap release honors it as an
                        // attack-move order (the old dead branch the §1.1
                        // audit found — the disarm used to precede the
                        // check), a world release disarms.
                        self.right_press_armed = self.attack_move_armed;
                        self.attack_move_armed = false;
                        self.right_button.press(cursor);
                        // Grab the ground under the press for the
                        // potential drag (None above the horizon; the
                        // crossing event re-grabs).
                        self.right_anchor = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1));
                    }
                    (MouseButton::Right, ElementState::Released) => {
                        // The release decides what the gesture was. A
                        // camera drag (or a swallowed press) orders nothing.
                        self.right_anchor = None;
                        let armed_at_press = self.right_press_armed;
                        self.right_press_armed = false;
                        match self.right_button.release(cursor) {
                            Some(RightRelease::Command) => {
                                // UI buttons swallow the right-click
                                // (right-clicking the command card is not
                                // a world order).
                                if self
                                    .ui_buttons
                                    .iter()
                                    .any(|button| button.contains(cursor.0 as f32, cursor.1 as f32))
                                {
                                    return;
                                }
                                // A minimap right-click orders at the
                                // mapped ground point (attack-move when
                                // the gesture began armed).
                                let minimap_hit = self.renderer.as_ref().and_then(|renderer| {
                                    renderer.minimap_hit(cursor.0 as f32, cursor.1 as f32)
                                });
                                if let Some((world_x, world_z)) = minimap_hit {
                                    if !self.selection.is_empty() {
                                        let target = MatchHost::world_to_logical(world_x, world_z);
                                        if armed_at_press {
                                            self.submit_order(
                                                CommandKind::AttackMove {
                                                    units: self.selection.clone(),
                                                    target,
                                                },
                                                Some(target),
                                            );
                                        } else {
                                            self.submit_order(
                                                CommandKind::Move {
                                                    units: self.selection.clone(),
                                                    target,
                                                },
                                                Some(target),
                                            );
                                        }
                                    }
                                    return;
                                }
                                self.issue_context_order(ndc);
                            }
                            Some(RightRelease::Scroll) | None => {
                                // The drag already scrolled; order nothing.
                            }
                        }
                    }
                    // M10.2 (PLAN §1.2): middle-drag rotates the camera
                    // (Generals does this — it panned before this pass).
                    // The press only anchors the cursor; CursorMoved turns
                    // horizontal travel into yaw.
                    (MouseButton::Middle, ElementState::Pressed) => {
                        self.middle_last = Some(cursor);
                    }
                    (MouseButton::Middle, ElementState::Released) => {
                        self.middle_last = None;
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                // Ctrl+wheel adjusts pitch (Generals-style); without Ctrl the
                // wheel zooms toward the cursor (plan §11.3, M9.1).
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, lines) => lines,
                    MouseScrollDelta::PixelDelta(delta) => delta.y as f32 * 0.05,
                };
                if lines == 0.0 {
                    return;
                }
                if self.modifiers.control_key() {
                    // Positive wheel = tilt down toward the ground; negative =
                    // raise toward the horizon. Generals pitch control.
                    self.camera.adjust_pitch(lines * PITCH_PER_WHEEL_NOTCH);
                } else {
                    // M9.1: wheel up zooms in, and the zoom keeps the cursor's
                    // ground anchor under the cursor (plan §11.3: "zoom toward
                    // the cursor" — the old orbit-only zoom read as the view
                    // sliding away, and the direction was inverted).
                    let ndc = self.cursor_ndc();
                    self.camera
                        .zoom_toward(1.0 - lines * 0.1, glam::Vec2::new(ndc.0, ndc.1));
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // M9.1: key repeats re-press an already-held key — they must
                // not re-fire hotkeys (holding S to pan spammed Stop orders
                // through every repeat; the DEBT-008 human pass).
                if event.repeat {
                    return;
                }
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        winit::keyboard::KeyCode::KeyW => set_key(&mut self.keys, "w", pressed),
                        // M9.1: the arrow keys alias the WASD pan (plan
                        // §11.3's key panning covers both habits).
                        winit::keyboard::KeyCode::ArrowUp => set_key(&mut self.keys, "w", pressed),
                        winit::keyboard::KeyCode::KeyA => {
                            set_key(&mut self.keys, "a", pressed);
                            // M9.1 (plan §11.3): 'A' arms attack-move; the
                            // next left-click places it (pressing the key
                            // no longer fires at the cursor — that fought
                            // the key's camera-pan role). Only while the
                            // match is ongoing; the end screen takes over.
                            if pressed && !self.host.is_finished() {
                                self.attack_move_armed = true;
                            }
                        }
                        winit::keyboard::KeyCode::ArrowLeft => {
                            set_key(&mut self.keys, "a", pressed)
                        }
                        winit::keyboard::KeyCode::KeyS => {
                            set_key(&mut self.keys, "s", pressed);
                            // M8: 'S' is also the Stop hotkey on press.
                            if pressed && !self.host.is_finished() {
                                self.issue_stop();
                            }
                        }
                        winit::keyboard::KeyCode::ArrowDown => {
                            set_key(&mut self.keys, "s", pressed)
                        }
                        winit::keyboard::KeyCode::KeyD => set_key(&mut self.keys, "d", pressed),
                        winit::keyboard::KeyCode::ArrowRight => {
                            set_key(&mut self.keys, "d", pressed)
                        }
                        // Generals-style camera rotation: Q orbits left, E
                        // orbits right. Held keys apply a continuous yaw rate
                        // scaled by the frame delta (about_to_wait). Both
                        // keys are also camera-only — they never fire commands
                        // and they cancel an armed attack-move (so the player
                        // can re-orient mid-order without losing it).
                        winit::keyboard::KeyCode::KeyQ => {
                            set_key(&mut self.keys, "q", pressed);
                            if pressed {
                                self.attack_move_armed = false;
                            }
                        }
                        winit::keyboard::KeyCode::KeyE => {
                            set_key(&mut self.keys, "e", pressed);
                            if pressed {
                                self.attack_move_armed = false;
                            }
                        }
                        // §11.6 debug tooling: overlay toggle, pause, single-step,
                        // and the M10.1 F8 bug-report dump (seed + tick,
                        // deterministic — see report.rs).
                        winit::keyboard::KeyCode::F3 if pressed => {
                            self.debug_overlay = !self.debug_overlay;
                        }
                        winit::keyboard::KeyCode::F8 if pressed => {
                            self.dump_bug_report();
                        }
                        winit::keyboard::KeyCode::KeyP if pressed => {
                            self.host.set_paused(!self.host.is_paused());
                        }
                        winit::keyboard::KeyCode::Period if pressed => {
                            let outcome = self.host.step_once();
                            self.collect_checkpoints(&outcome);
                        }
                        // M8 (plan §11.3): control groups 1-9. Ctrl+digit
                        // assigns the current selection; digit alone
                        // recalls. M10.2 (PLAN §1.3): a double tap of the
                        // same digit within DOUBLE_TAP_MS recalls AND
                        // centers the camera on the group's live members.
                        // Digit 0 is unused (9 groups, 1-9).
                        winit::keyboard::KeyCode::Digit1
                        | winit::keyboard::KeyCode::Digit2
                        | winit::keyboard::KeyCode::Digit3
                        | winit::keyboard::KeyCode::Digit4
                        | winit::keyboard::KeyCode::Digit5
                        | winit::keyboard::KeyCode::Digit6
                        | winit::keyboard::KeyCode::Digit7
                        | winit::keyboard::KeyCode::Digit8
                        | winit::keyboard::KeyCode::Digit9
                            if pressed =>
                        {
                            let index = digit_to_group_index(code) - 1;
                            if self.modifiers.control_key() {
                                self.assign_control_group(index);
                                // An assign breaks the double-tap chain —
                                // the next double tap must be two recalls.
                                self.last_digit_press = None;
                            } else {
                                let now = Instant::now();
                                let double_tap =
                                    self.last_digit_press.is_some_and(|(last_index, then)| {
                                        last_index == index
                                            && input::is_double_tap(
                                                now.duration_since(then).as_millis() as u64,
                                            )
                                    });
                                self.recall_control_group(index);
                                if double_tap {
                                    // Center the camera on the group's
                                    // live members (dead ids weigh nothing).
                                    if let Some((x, z)) = input::group_center(
                                        &self.view_snapshot().entities,
                                        &self.control_groups[index],
                                    ) {
                                        let distance = self.camera.distance();
                                        self.camera.focus(x, z, distance);
                                    }
                                }
                                self.last_digit_press = Some((index, now));
                            }
                        }
                        // M8 (plan §9.7, A15): 'R' restarts the match when
                        // it has ended. The host's outcome surfaces through
                        // the boundary; restart drops + reconstructs.
                        winit::keyboard::KeyCode::KeyR if pressed => {
                            if self.host.is_finished() {
                                self.restart();
                            }
                        }
                        winit::keyboard::KeyCode::Escape if pressed => {
                            // M9.1: Esc cancels an armed attack-move first,
                            // then placement; with nothing armed it quits.
                            if self.attack_move_armed {
                                self.attack_move_armed = false;
                            } else if self.placement.take().is_some() {
                                // placement cancelled
                            } else {
                                event_loop.exit();
                            }
                        }
                        _ => {}
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            WindowEvent::Focused(focused) => {
                self.focused = focused;
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // M10.1 (plan §6.5): `--record`'s clean-exit write. Every exit path
        // (window close, Escape, the `--frames` budget) lands here — the
        // segment's replay is written unless the match already ended and
        // wrote its record at the end tick.
        if self.record_path.is_some() && !self.record_written {
            self.write_recording();
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // M9.1: camera motion from held keys and the screen edge, scaled by
        // the real frame delta (frame-rate independent — the old fixed
        // per-frame speed made pan speed a property of the monitor).
        let dt = self
            .last_frame
            .elapsed()
            .as_secs_f32()
            .min(MAX_PAN_FRAME_SECONDS);
        let pan = PAN_TILES_PER_SECOND * dt;
        let mut dx = 0.0;
        let mut dz = 0.0;
        // M9.1: W pans the view up-screen (away from the camera), D pans
        // right — the M3 signs were mirrored along with the camera's right
        // axis (the DEBT-008 human pass: "d goes left and a goes right").
        if self.keys.contains(&Key::Character("w")) {
            dz += pan;
        }
        if self.keys.contains(&Key::Character("s")) {
            dz -= pan;
        }
        if self.keys.contains(&Key::Character("a")) {
            dx -= pan;
        }
        if self.keys.contains(&Key::Character("d")) {
            dx += pan;
        }
        // M10.2 (PLAN §1.2): edge scrolling — a depth-scaled band instead
        // of the old binary zone. The cursor at the window edge scrolls at
        // full speed, halfway into the `input::EDGE_SCROLL_BAND_PX` band at
        // half speed, and the interior not at all; all four edges work and
        // corners combine (the vector carries both axes). Focused windows
        // only — an unfocused game never scrolls the desktop. The band is
        // toggleable (`edge_scroll_enabled`); the settings screen that
        // flips it is Phase 3.
        if self.focused && self.edge_scroll_enabled {
            if let (Some(window), Some((mx, my))) = (&self.window, self.drag_current) {
                let size = window.inner_size();
                if size.width > 0 && size.height > 0 {
                    let (scale_x, scale_z) = input::edge_scroll_vector(
                        (mx, my),
                        (size.width as f64, size.height as f64),
                    );
                    dx += pan * scale_x;
                    dz += pan * scale_z;
                }
            }
        }
        if dx != 0.0 || dz != 0.0 {
            self.camera.pan(dx, dz);
        }
        // Generals-style camera rotation (Q/E): a continuous yaw rate, scaled
        // by the frame delta — frame-rate-independent, like the pan above.
        let mut yaw_delta = 0.0;
        if self.keys.contains(&Key::Character("q")) {
            yaw_delta -= ROTATE_RAD_PER_SEC * dt;
        }
        if self.keys.contains(&Key::Character("e")) {
            yaw_delta += ROTATE_RAD_PER_SEC * dt;
        }
        if yaw_delta != 0.0 {
            self.camera.rotate(yaw_delta);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    /// The windowed smoke summary: what the `--frames` run proved (frames
    /// presented, sim progress, commands submitted through input, selection,
    /// and — the M9 full-loop evidence — whether the match resolved).
    fn windowed_summary(&self) {
        let outcome = match self.host.outcome() {
            Some(outcome) => format!(
                "match ended, {} wins (tick {})",
                match outcome.winner {
                    PlayerId(id) => format!("player {id}"),
                },
                outcome.ended_tick
            ),
            None => "match ongoing".to_string(),
        };
        println!(
            "pandemonium client — windowed smoke: {} frames presented, tick {}, state hash {:#018x}, \
             {} commands submitted, selection {}, {}",
            self.frames_presented,
            self.host.tick(),
            self.host.state_hash(),
            self.commands_submitted,
            self.selection.len(),
            outcome
        );
    }

    fn draw(&mut self) {
        let dt = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        let outcome = self.host.advance(dt);
        // M10.1 (plan §6.5/§11.6): the trail feeding --record and F8.
        self.collect_checkpoints(&outcome);
        // M10.1 (plan §6.5): --record writes the segment's replay at the
        // match's end — the log is complete there, and the checkpoint at the
        // end tick is the natural replay finale.
        if self.record_path.is_some() && !self.record_written && self.host.is_finished() {
            self.write_recording();
        }
        // M9 (plan §11.5, FD-9): the step's events flow outward — the audio
        // sink and the feedback state consume them after the step, never
        // inside it. Presentation-only bookkeeping.
        self.audio.on_events(&outcome.events);
        self.feedback
            .on_events(&outcome.events, self.frames_presented);
        // M9.1: a refused order surfaces — the red square at its click
        // point and the HUD line with the reason. The human pass found
        // silent rejections read as "the controls don't work"; the client
        // knows where each order was clicked, so the refusal lands there.
        // Deaths become shrinking, charring cues at the entity's last-seen
        // spot (the event carries only the id; last_seen has the rest).
        for event in &outcome.events {
            match event {
                Event::CommandRejected {
                    issuer,
                    seq,
                    reject,
                } => {
                    if *issuer == HUMAN
                        && self
                            .last_order
                            .is_some_and(|(last_seq, _)| last_seq == *seq)
                    {
                        let (_, at) = self.last_order.expect("just checked");
                        self.feedback
                            .refuse(at, reject.reason, self.frames_presented);
                    }
                }
                Event::Died { entity } => {
                    if let Some(&(pos, kind, owner)) = self.last_seen.get(entity) {
                        self.feedback.death(pos, kind, owner, self.frames_presented);
                    }
                }
                _ => {}
            }
        }
        self.feedback.expire(self.frames_presented);
        // The fog-filtered view (FD-8 from the rendering side): the human
        // renders and clicks through what their player may see. The view is
        // pushed at the same cadence the host pushes its full snapshots, and
        // the fog texture refreshes once per sim step, not per frame.
        let view = self.host.player_view(HUMAN);
        let view_tick = view.tick;
        let ore = view
            .resources
            .first()
            .map(|resource| resource.amount)
            .unwrap_or(0);
        self.view_interp.push(Snapshot {
            tick: view.tick,
            entities: view.entities.clone(),
        });
        let fog_changed = self.fog_tick != view_tick as u64;
        let fog_bytes: Vec<u8> = if fog_changed {
            view.fog
                .iter()
                .map(|tile| match tile {
                    TileFog::Hidden => render::FOG_ALPHA_HIDDEN,
                    TileFog::Explored => render::FOG_ALPHA_EXPLORED,
                    TileFog::Visible => render::FOG_ALPHA_VISIBLE,
                })
                .collect()
        } else {
            Vec::new()
        };
        self.latest_view = Some(view);
        let snapshot = self.view_snapshot();
        // Remember where everything visible stands this frame — the death
        // fade's source of last-seen positions (kept after the event loop
        // so a spawn-and-die-in-one-tick entity still fades where it stood).
        self.last_seen
            .retain(|id, _| snapshot.entities.iter().any(|entity| entity.id == *id));
        for entity in &snapshot.entities {
            self.last_seen
                .insert(entity.id, (entity.pos, entity.kind, entity.owner));
        }
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        if fog_changed {
            if fog_bytes != self.fog_bytes {
                renderer.update_fog(&fog_bytes);
                self.fog_bytes = fog_bytes;
            }
            self.fog_tick = view_tick as u64;
        }
        let view_projection = self.camera.view_projection();
        let eye = self.camera.eye();
        // The HUD's `state_hash` is only read inside the F3 debug overlay,
        // so we use the cheap hud_state path every frame and only pay for
        // the canonical hash when the overlay is actually visible. The
        // hash is O(n) over the whole world — paying it every frame for a
        // value nothing else reads is a real waste on the renderer's hot
        // path.
        let hud = if self.debug_overlay {
            self.host.hud_state_with_hash(HUMAN)
        } else {
            self.host.hud_state(HUMAN)
        };
        let outcome = self.host.outcome();
        // M9: the health bars and command pings join the overlay pass
        // (plan §11.2's overlay layer). Projected through the same camera
        // the box select uses.
        let viewport = self
            .window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width as f32, size.height as f32)
            })
            .unwrap_or((1920.0, 1080.0));
        // M9.1: the selection brackets — the persistent read of what is
        // selected (the brightened team color alone did not read at play
        // distance).
        let mut quads = feedback::selection_quads(
            renderer.atlas(),
            &self.selection,
            &snapshot,
            &self.camera,
            viewport,
        );
        quads.extend(feedback::health_bar_quads(
            renderer.atlas(),
            &snapshot,
            &self.camera,
            viewport,
        ));
        quads.extend(feedback::ping_quads(
            renderer.atlas(),
            self.feedback.active_pings(),
            &self.camera,
            self.frames_presented,
            viewport,
        ));
        // M9.1: the refusal squares.
        quads.extend(feedback::refusal_quads(
            renderer.atlas(),
            self.feedback.active_refusals(),
            &self.camera,
            self.frames_presented,
            viewport,
        ));
        // The HUD and debug overlay quads (plan §11.4 / §11.6) queue before the
        // frame; the renderer drains them on top of the world.
        quads.extend(overlay_quads(OverlayInput {
            atlas: renderer.atlas(),
            hud: &hud,
            resource_names: &self.resource_names,
            debug: self.debug_overlay,
            selected: self.selection.len(),
            entities: snapshot.entities.len(),
            outcome: outcome.as_ref(),
            log_len: self.host.log().len(),
            viewport,
            armed: self.attack_move_armed,
            notice: self
                .feedback
                .refusal_notice
                .as_ref()
                .map(|(text, _)| text.as_str()),
        }));
        // The bottom bar (plan §11.4): selection panel, command card, and
        // queue strip, rebuilt every frame; its buttons are hit-tested in
        // the input pass before any world click.
        self.ui_buttons.clear();
        if let Some(view) = &self.latest_view {
            let layout = ui::bar_layout(viewport);
            let built = ui::build_bottom_bar(ui::CardInput {
                atlas: renderer.atlas(),
                layout,
                view_entities: &view.entities,
                production: &view.production,
                bundle: &self.bundle,
                selection: &self.selection,
                ore,
            });
            quads.extend(built.quads);
            self.ui_buttons = built.buttons;
            // The minimap: composite this frame's texture, size the quad,
            // and draw the border + camera viewport indicator on top (UI
            // quads land over the minimap pass).
            renderer.set_minimap_rect(layout.minimap);
            renderer.update_minimap(&snapshot.entities, &self.fog_bytes);
            quads.extend(panel_border(
                renderer.atlas(),
                layout.minimap,
                [0.85, 0.88, 0.92, 0.5],
            ));
            quads.extend(minimap_viewport_quads(
                renderer.atlas(),
                &self.camera,
                (self.bundle.map.width as f32, self.bundle.map.height as f32),
                layout.minimap,
            ));
            // The placement ghost (plan §11.3's legality preview): a tinted
            // footprint quad under the cursor — green when the client-side
            // preview likes it, red when it does not (the gate re-checks).
            renderer.set_ghost(self.placement.as_ref().and_then(|placement| {
                let (tile_x, tile_y) = placement.ghost_tile?;
                let legal = ui::placement_is_likely_legal(
                    &self.bundle,
                    &view.entities,
                    placement.structure,
                    TilePos {
                        x: tile_x,
                        y: tile_y,
                    },
                );
                let (w, h) = self
                    .bundle
                    .entities
                    .get(placement.structure.0 as usize)
                    .and_then(|def| def.footprint())
                    .unwrap_or((1, 1));
                Some(render::PlacementGhost {
                    center: [
                        tile_x as f32 + w as f32 * 0.5,
                        0.0,
                        tile_y as f32 + h as f32 * 0.5,
                    ],
                    extent: [w as f32, h as f32],
                    legal,
                })
            }));
        }
        // Everything the overlay pass draws this frame queues together: the
        // feedback cues, the HUD panels, and the bottom bar.
        renderer.queue_ui(&quads);
        // The death fades: one shrinking, charring instance per active cue,
        // with the progress from the cue's remaining frames.
        renderer.set_death_cues(
            self.feedback
                .active_deaths()
                .iter()
                .map(|cue| render::DyingCue {
                    pos: [cue.pos.x, 0.0, cue.pos.z],
                    kind: cue.kind,
                    owner: cue.owner,
                    progress: 1.0
                        - ((cue.expires_at - self.frames_presented.min(cue.expires_at)) as f32
                            / feedback::DEATH_FRAMES as f32),
                })
                .collect(),
        );
        let flashes: Vec<EntityId> = snapshot
            .entities
            .iter()
            .map(|entity| entity.id)
            .filter(|id| self.feedback.is_flashing(*id))
            .collect();
        renderer.render(Frame {
            snapshot: &snapshot,
            view_projection,
            eye,
            selection: &self.selection,
            hud: &hud,
            flashes: &flashes,
        });
    }
}

/// Draws a thin rectangular border around a panel rect (UI quads).
fn panel_border(atlas: &TextAtlas, rect: [f32; 4], color: [f32; 4]) -> Vec<UiQuad> {
    const T: f32 = 1.5;
    let [x, y, w, h] = rect;
    let mut quads = vec![
        atlas.solid_rect(x - T, y - T, w + 2.0 * T, T, color),
        atlas.solid_rect(x - T, y + h, w + 2.0 * T, T, color),
        atlas.solid_rect(x - T, y, T, h, color),
        atlas.solid_rect(x + w, y, T, h, color),
    ];
    quads.shrink_to_fit();
    quads
}

/// The camera's viewport indicator on the minimap: the view frustum's
/// ground footprint (clamped to the map) drawn as a dotted outline. Dots
/// rather than a stroked rect keep the quads axis-aligned (the footprint
/// rotates with the camera's yaw).
fn minimap_viewport_quads(
    atlas: &TextAtlas,
    camera: &RtsCamera,
    map: (f32, f32),
    rect: [f32; 4],
) -> Vec<UiQuad> {
    const DOT: f32 = 2.0;
    const STEP_PX: f32 = 6.0;
    let to_px = |p: glam::Vec3| -> (f32, f32) {
        let x = (p.x.clamp(0.0, map.0) / map.0) * rect[2] + rect[0];
        let y = (p.z.clamp(0.0, map.1) / map.1) * rect[3] + rect[1];
        (x, y)
    };
    let corners = camera.ground_footprint_corners();
    let pts: Vec<(f32, f32)> = corners.iter().map(|p| to_px(*p)).collect();
    let mut quads = Vec::new();
    for index in 0..4 {
        let (x0, y0) = pts[index];
        let (x1, y1) = pts[(index + 1) % 4];
        let length = ((x1 - x0).powi(2) + (y1 - y0).powi(2)).sqrt();
        let steps = (length / STEP_PX).ceil().max(1.0) as usize;
        for step in 0..=steps {
            let t = step as f32 / steps as f32;
            let x = x0 + (x1 - x0) * t;
            let y = y0 + (y1 - y0) * t;
            quads.push(atlas.solid_rect(
                x - DOT * 0.5,
                y - DOT * 0.5,
                DOT,
                DOT,
                [1.0, 1.0, 1.0, 0.85],
            ));
        }
    }
    quads
}

/// The inputs to [`overlay_quads`] for one frame (M8: gathered into a struct
/// to keep the call readable as the panel set grew). All presentation-only.
struct OverlayInput<'a> {
    atlas: &'a TextAtlas,
    hud: &'a HudState,
    resource_names: &'a [String],
    debug: bool,
    selected: usize,
    entities: usize,
    outcome: Option<&'a MatchOutcome>,
    log_len: usize,
    /// The window's pixel size (M9.1: the end screen centers in the actual
    /// viewport, not a guessed 320x240).
    viewport: (f32, f32),
    /// M9.1: whether an attack-move is armed (the HUD instruction line).
    armed: bool,
    /// M9.1: the refusal notice line, when a recent order was refused.
    notice: Option<&'a str>,
}

/// Builds the overlay quads for one frame: the resource/population HUD
/// (plan §11.4's minimal slice), the §11.6 debug overlay (when toggled), and
/// (M8) the end-screen panel when the match has resolved.
fn overlay_quads(input: OverlayInput<'_>) -> Vec<UiQuad> {
    let OverlayInput {
        atlas,
        hud,
        resource_names,
        debug,
        selected,
        entities,
        outcome,
        log_len,
        viewport,
        armed,
        notice,
    } = input;
    /// Screen margin between panels and the window edge.
    const MARGIN: f32 = 8.0;
    /// Padding inside a panel.
    const PAD: f32 = 6.0;
    let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
    let mut quads = Vec::new();

    // The resource line: every ledger entry by its data-defined display name.
    let mut line = String::new();
    for (index, (resource, amount)) in hud.resources.iter().enumerate() {
        if index > 0 {
            line.push_str("   ");
        }
        let name = resource_names
            .get(resource.0 as usize)
            .map(String::as_str)
            .unwrap_or("RES");
        line.push_str(&format!("{name} {amount}"));
    }
    line.push_str(&format!("   POP {}/{}", hud.population, hud.population_cap));
    let text_width = atlas.measure(&line);
    let baseline = MARGIN + PAD + atlas.ascent;
    quads.push(atlas.solid_rect(
        MARGIN,
        MARGIN,
        text_width + 2.0 * PAD,
        line_height + 2.0 * PAD,
        [0.0, 0.0, 0.0, 0.55],
    ));
    quads.extend(atlas.layout(&line, MARGIN + PAD, baseline, [1.0, 1.0, 1.0, 0.95]));

    // M9.1: the armed attack-move instruction (amber) and the refusal
    // notice (red), directly under the resource line — one line each, so
    // the player always knows what the next click will do.
    let mut notice_top = MARGIN + line_height + 2.0 * PAD + MARGIN;
    if armed {
        let line = "ATTACK MOVE - left-click a target (Esc cancels)";
        let width = atlas.measure(line);
        quads.push(atlas.solid_rect(
            MARGIN,
            notice_top,
            width + 2.0 * PAD,
            line_height + 2.0 * PAD,
            [0.25, 0.18, 0.02, 0.55],
        ));
        quads.extend(atlas.layout(
            line,
            MARGIN + PAD,
            notice_top + PAD + atlas.ascent,
            [1.0, 0.8, 0.3, 0.95],
        ));
        notice_top += line_height + 2.0 * PAD + MARGIN;
    }
    if let Some(notice) = notice {
        let width = atlas.measure(notice);
        quads.push(atlas.solid_rect(
            MARGIN,
            notice_top,
            width + 2.0 * PAD,
            line_height + 2.0 * PAD,
            [0.22, 0.03, 0.03, 0.55],
        ));
        quads.extend(atlas.layout(
            notice,
            MARGIN + PAD,
            notice_top + PAD + atlas.ascent,
            [1.0, 0.45, 0.4, 0.95],
        ));
    }

    // The debug overlay (§11.6), below the notice lines.
    if debug {
        let lines = [
            format!("tick {}", hud.tick),
            format!("hash {:#018x}", hud.state_hash),
            format!("entities {entities}  selected {selected}  log {log_len}"),
            if hud.paused {
                "PAUSED  [.] step  [F8] bug report".to_string()
            } else {
                "[F3] overlay  [P] pause  [F8] bug report".to_string()
            },
        ];
        let widest = lines
            .iter()
            .map(|line| atlas.measure(line))
            .fold(0.0f32, f32::max);
        let top = notice_top;
        let panel_height = lines.len() as f32 * line_height + 2.0 * PAD;
        quads.push(atlas.solid_rect(
            MARGIN,
            top,
            widest + 2.0 * PAD,
            panel_height,
            [0.0, 0.0, 0.0, 0.45],
        ));
        for (index, line) in lines.iter().enumerate() {
            let line_baseline = top + PAD + atlas.ascent + index as f32 * line_height;
            let color = if line.starts_with("PAUSED") {
                [1.0, 0.85, 0.3, 0.95]
            } else {
                [0.85, 0.92, 1.0, 0.95]
            };
            quads.extend(atlas.layout(line, MARGIN + PAD, line_baseline, color));
        }
    } else if hud.paused {
        // A visible PAUSED tag even with the overlay hidden: the same row as
        // the resource panel, just to its right.
        let tag = "PAUSED";
        let width = atlas.measure(tag);
        let tag_x = MARGIN + text_width + 2.0 * PAD + MARGIN;
        quads.push(atlas.solid_rect(
            tag_x,
            MARGIN,
            width + 2.0 * PAD,
            line_height + 2.0 * PAD,
            [0.2, 0.15, 0.0, 0.45],
        ));
        quads.extend(atlas.layout(tag, tag_x + PAD, baseline, [1.0, 0.85, 0.3, 0.95]));
    }

    // M8: the end-screen panel (plan §11.4: "match end screen with restart").
    // Rendered on top of everything when the match has resolved. The human's
    // slot is 0; the AI's is 1; NEUTRAL is the mutual-destruction edge case.
    if let Some(outcome) = outcome {
        let headline = match outcome.winner {
            HUMAN => "VICTORY",
            AI => "DEFEAT",
            _ => "MUTUAL DESTRUCTION",
        };
        let subline = match outcome.winner {
            HUMAN => "You eliminated the enemy.",
            AI => "Your base was destroyed.",
            _ => "Both sides were eliminated.",
        };
        let hint = "[R] restart    [Esc] quit";
        let lines = [headline, subline, hint];
        let widest = lines
            .iter()
            .map(|l| atlas.measure(l))
            .fold(0.0f32, f32::max);
        let panel_w = widest + 4.0 * PAD;
        let panel_h = lines.len() as f32 * line_height + 4.0 * PAD;
        // M9.1: center the panel in the actual window (the old fixed
        // 320x240 guess only centered the default window size).
        let panel_x = viewport.0 * 0.5 - panel_w * 0.5;
        let panel_y = viewport.1 * 0.5 - panel_h * 0.5;
        let panel_color = if outcome.winner == HUMAN {
            [0.05, 0.12, 0.05, 0.85]
        } else {
            [0.18, 0.04, 0.04, 0.85]
        };
        quads.push(atlas.solid_rect(panel_x, panel_y, panel_w, panel_h, panel_color));
        for (index, line) in lines.iter().enumerate() {
            let line_baseline = panel_y + 2.0 * PAD + atlas.ascent + index as f32 * line_height;
            let color = if index == 0 {
                if outcome.winner == HUMAN {
                    [0.6, 1.0, 0.6, 1.0]
                } else {
                    [1.0, 0.5, 0.5, 1.0]
                }
            } else {
                [1.0, 1.0, 1.0, 0.9]
            };
            let line_w = atlas.measure(line);
            let line_x = panel_x + (panel_w - line_w) / 2.0;
            quads.extend(atlas.layout(line, line_x, line_baseline, color));
        }
    }
    quads
}

/// Builds the terrain mesh from the content the client loaded: the map grid
/// plus the display-only heightmap (ADR-0001). Revalidates the bundle, which
/// already passed at startup, so it cannot fail in practice.
fn terrain_mesh_of() -> pandemonium_engine::mesh::TerrainMesh {
    let bundle = ContentBundle::load_dir(content_path()).expect("content revalidates");
    terrain_mesh(&bundle.map, HEIGHT_SCALE)
}

/// The headless smoke pass: prove the whole pipeline minus the GPU — content
/// loads, the match advances, the renderer interface is driven (HUD data
/// included), and the state hash is stable.
fn headless_smoke(bundle: &ContentBundle) -> anyhow::Result<()> {
    let setup = MatchSetup {
        seed: 7,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Ai,
            },
        ],
    };
    // M9: the smoke drives the windowed path's exact hosting seam — the AI
    // opponent runs through MatchHost::with_controllers, so the event wiring
    // below sees the same stream the windowed client draws from.
    let mut host = App::build_host(bundle, setup);
    let mut null_renderer = NullRenderer::new();
    // M9: the smoke run exercises the feel pass's event wiring too — the
    // same sink + feedback state the windowed draw feeds, driven without a
    // display (their counters are the printed evidence).
    let mut audio = NullAudioSink::new();
    let mut feedback = FeedbackState::default();
    // M9.1: the smoke also exercises the refusal wiring — one deliberately
    // invalid order (a Move naming a unit that does not exist) must come
    // back as a CommandRejected the client can see, and the feedback
    // layer turns it into the red-square cue + HUD notice the windowed
    // path draws. The same loop below surfaces it.
    host.submit(Command::new(
        HUMAN,
        0,
        1,
        CommandKind::Move {
            units: vec![EntityId(999_999)],
            target: Vec2Fx::from_ints(12, 12),
        },
    ));
    let frame_dt = std::time::Duration::from_secs_f64(1.0 / 60.0);
    let mut entities = 0;
    let mut refusal_surfaced = false;
    for frame in 0..180 {
        let outcome = host.advance(frame_dt);
        audio.on_events(&outcome.events);
        feedback.on_events(&outcome.events, frame);
        for event in &outcome.events {
            if let Event::CommandRejected {
                issuer,
                seq,
                reject,
            } = event
            {
                if *issuer == HUMAN && *seq == 1 {
                    feedback.refuse(Vec2Fx::from_ints(12, 12), reject.reason, frame);
                    refusal_surfaced = true;
                    // Set the moment the refusal lands (it expires frames
                    // later by design — that is the cue's lifetime).
                    assert!(
                        feedback.refusal_notice.is_some(),
                        "the refusal notice line is set for the HUD"
                    );
                }
            }
        }
        feedback.expire(frame);
        let snapshot = host.render_snapshot();
        entities = snapshot.entities.len();
        // The HUD plumbing rides along (gathered through the boundary, drawn by
        // nothing — the null renderer accepts and discards).
        let hud = host.hud_state(HUMAN);
        let flashes: Vec<EntityId> = snapshot
            .entities
            .iter()
            .map(|entity| entity.id)
            .filter(|id| feedback.is_flashing(*id))
            .collect();
        null_renderer.render(Frame {
            snapshot: &snapshot,
            view_projection: glam::Mat4::IDENTITY,
            eye: glam::Vec3::ZERO,
            selection: &[],
            flashes: &flashes,
            hud: &hud,
        });
    }
    let hud = host.hud_state(HUMAN);
    println!(
        "  headless smoke: 180 frames, {entities} entities, tick {}, state hash {:#018x}",
        host.tick(),
        host.state_hash()
    );
    println!(
        "  hud at exit:    tick {}, paused {}, pop {}/{}",
        hud.tick, hud.paused, hud.population, hud.population_cap
    );
    println!(
        "  feedback wiring: {} events fed the audio sink ({} cues), {} attacks flashed on screen, \
         {} refused orders surfaced",
        audio.fed, audio.cues, feedback.hits_seen, feedback.refusals_seen
    );
    assert!(
        refusal_surfaced && feedback.refusals_seen > 0,
        "the invalid order must have surfaced as a refusal cue"
    );
    println!("  content hash:    {:#018x}", bundle.content_hash());
    println!("pandemonium client — smoke PASS (windowed M3 verification requires a display)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{frames_budget_arg, record_from_args, seed_from_args};

    /// `args(["--seed", "42"])` — the standard helper spelling.
    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn seed_parser_accepts_a_valid_value() {
        assert_eq!(
            seed_from_args(&args(&["pandemonium-client", "--seed", "42"])),
            42
        );
        assert_eq!(seed_from_args(&args(&["--seed", "0"])), 0);
        assert_eq!(
            seed_from_args(&args(&["--seed", "18446744073709551615"])),
            u64::MAX
        );
    }

    #[test]
    fn seed_parser_defaults_without_the_flag() {
        assert_eq!(seed_from_args(&args(&["pandemonium-client"])), 7);
        assert_eq!(seed_from_args(&args(&["--frames", "900"])), 7);
    }

    #[test]
    fn seed_parser_falls_back_on_a_missing_value() {
        // Mirrors `--frames`: a missing value silently takes the default
        // (7) rather than failing to start — the flag is an affordance, not
        // a contract.
        assert_eq!(seed_from_args(&args(&["--seed"])), 7);
    }

    #[test]
    fn seed_parser_falls_back_on_a_garbage_value() {
        assert_eq!(seed_from_args(&args(&["--seed", "not-a-number"])), 7);
        assert_eq!(seed_from_args(&args(&["--seed", "-1"])), 7);
        assert_eq!(seed_from_args(&args(&["--seed", ""])), 7);
    }

    #[test]
    fn record_parser_reads_the_path() {
        assert_eq!(
            record_from_args(&args(&["--record", "/tmp/match.pdrp"])),
            Some(std::path::PathBuf::from("/tmp/match.pdrp"))
        );
        // A relative path (the playtest quickstart's shape) passes verbatim.
        assert_eq!(
            record_from_args(&args(&["--record", "match.pdrp"])),
            Some(std::path::PathBuf::from("match.pdrp"))
        );
    }

    #[test]
    fn record_parser_is_none_without_the_flag_or_value() {
        assert_eq!(record_from_args(&args(&["pandemonium-client"])), None);
        assert_eq!(record_from_args(&args(&["--seed", "42"])), None);
        // A missing value disables recording rather than guessing a file.
        assert_eq!(record_from_args(&args(&["--record"])), None);
    }

    #[test]
    fn frames_parser_still_parses_after_the_signature_change() {
        assert_eq!(frames_budget_arg(&args(&["--frames", "900"])), Some(900));
        assert_eq!(
            frames_budget_arg(&args(&["--frames", "garbage"])),
            Some(180)
        );
        assert_eq!(frames_budget_arg(&args(&["pandemonium-client"])), None);
    }
}
