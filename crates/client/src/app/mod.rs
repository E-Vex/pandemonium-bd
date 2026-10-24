// `app/` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// non-input application state — the `App` struct, match hosting + restart,
// the checkpoint trail, the `--record`/F11 wiring (`report.rs` stays the pure
// seam), the menu/draw frame assembly, and the HUD quad builders. `main.rs`
// remains the winit bootstrap; the event translation lives in `input/`
// (ADR step 7).
//
// Visibility note: the fields and methods are `pub(crate)` because the
// crate's other halves — `main.rs`'s `ApplicationHandler` impl and its test
// modules, and (after ADR step 7) the `input/` event translation — are
// siblings of this module. This is the split's plumbing, not an API change:
// the crate is a binary, nothing is externally published, and no signature
// (parameter or return type) moves.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::text::{TextAtlas, UiQuad};
use crate::ui::{Button, MenuButton};
use pandemonium_ai::Controller;
use pandemonium_content::{ContentBundle, ContentTree};
use pandemonium_engine::audio::{AudioCue, AudioSink};
use pandemonium_engine::mesh::terrain_mesh;
use pandemonium_engine::renderer::{Frame, HudState};
use pandemonium_engine::{
    alpha_controller, alpha_plan, FrameOutcome, Interpolator, MatchHost, MatchOutcome, Renderer,
    RtsCamera,
};
use pandemonium_sim_api::{
    EntityId, Event, KindId, MatchSetup, MissionData, PlayerId, PlayerView, Snapshot, TileFog,
    Vec2Fx,
};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, ModifiersState};
use winit::window::Window;

use crate::config;
use crate::feedback;
use crate::feedback::FeedbackState;
use crate::input;
use crate::input::orders;
use crate::input::RightButton;
use crate::pack::AssetPack;
use crate::random_seed;
use crate::render;
use crate::render::WgpuRenderer;
use crate::report;
use crate::screens::{AppState, Effect, KeyContext, MatchMode, NavKey, Screen};
use crate::silhouette;
use crate::sound;
use crate::ui;
use crate::START_CAMERA_DISTANCE;
use crate::{AI, CONTROL_GROUP_COUNT, HUMAN};
use pandemonium_sim_api::TilePos;

/// The windowed application state.
pub(crate) struct App {
    /// The whole loaded content tree (PLAN-M10.2 §3.3): the New Match screen
    /// lists its maps and a started match bundles the selected one.
    pub(crate) tree: ContentTree,
    /// The current screen state (PLAN-M10.2 §3.1) — the application state
    /// machine plus the keyboard focus within the active menu.
    pub(crate) screen: AppState,
    /// The pre-filled seed (the `--seed N` flag; None = the New Match field
    /// starts random — A-111's OS-entropy source).
    pub(crate) seed_prefill: Option<u64>,
    /// The persisted settings (PLAN-M10.2 §3.4), loaded at startup with safe
    /// defaults when the config file is missing or malformed — the settings
    /// screen edits this live copy and saves it on Done.
    pub(crate) settings: config::Settings,
    /// The loaded client-side asset pack (presentation-only, local to this
    /// client). `None` when no pack exists or it cannot be used — the
    /// renderer falls back to placeholders. Never enters the simulation.
    pub(crate) pack: Option<AssetPack>,
    /// The current match's bundle (rebuilt per match: the selected map's).
    /// At the menu this holds the default bundle (the backdrop's map).
    pub(crate) bundle: ContentBundle,
    /// The live match's host — None until Start is pressed (PLAN-M10.2 §3.2:
    /// the game starts at the menu), Some for exactly as long as the screen
    /// is InMatch / PauseMenu / EndScreen. Every host access is guarded on
    /// this; a menu-only session must never panic, write a report, or
    /// submit a command.
    pub(crate) host: Option<MatchHost>,
    /// The live match's mode (A-112) — what Start was pressed with.
    pub(crate) match_mode: MatchMode,
    /// The live match's setup (M8: retained for restart — the same seed
    /// reproduces the same match bit-for-bit, A15).
    pub(crate) setup: MatchSetup,
    /// The live match's selected map (index into `tree.maps`).
    pub(crate) map_index: usize,
    /// A-011 (ADR-0201 Phase 1): the live match's mission, when it is a
    /// mission match — the tree's mission list index Start was pressed
    /// with. Restart rebuilds the same mission through it (the A15 shape:
    /// same mission, same seed, same match); a mission-less match keeps
    /// `None`.
    pub(crate) mission_index: Option<usize>,
    /// A-011: the live mission's presentation state — the HUD's objective
    /// lines and the end screen's mission text read this one tracker, fed
    /// by `Event::ObjectiveState` after the step (FD-9). `None` on every
    /// mission-less match: the HUD and the end screen stay exactly as they
    /// were.
    pub(crate) mission_hud: Option<ui::MissionHud>,
    /// A-011 (ADR-0201 §5, option (i)): the active live-state cutscene, if
    /// any — the host is paused while it runs, the camera pans over the
    /// frozen live world, and any key ends the scene and unpauses. `None`
    /// outside a scene.
    pub(crate) cutscene: Option<crate::cutscene::Cutscene>,
    pub(crate) camera: RtsCamera,
    pub(crate) renderer: Option<WgpuRenderer>,
    pub(crate) window: Option<Arc<Window>>,
    pub(crate) selection: Vec<EntityId>,
    /// M8 (plan §11.3) + PLAN-M11 §2: control groups 1-10 (Digit0 is the
    /// tenth). Ctrl+digit assigns the current selection; digit alone
    /// recalls it; Alt+digit views it without selecting.
    pub(crate) control_groups: Vec<Vec<EntityId>>,
    pub(crate) drag_start: Option<(f64, f64)>,
    pub(crate) drag_current: Option<(f64, f64)>,
    pub(crate) keys: BTreeSet<Key<&'static str>>,
    /// M8: the current keyboard modifiers (Ctrl for control-group assignment).
    pub(crate) modifiers: ModifiersState,
    /// M10.2 (PLAN §1.2): the right button's disambiguation state — a
    /// press-and-release under [`input::RIGHT_DRAG_COMMAND_MAX_PX`] is the
    /// command click; travel beyond it is the Generals-style grab-and-drag
    /// map scroll (and the release then orders nothing).
    pub(crate) right_button: RightButton,
    /// M10.2 (PLAN §1.2): the ground point grabbed at the right-button
    /// press — the anchor the drag keeps under the cursor while scrolling.
    pub(crate) right_anchor: Option<glam::Vec3>,
    /// M10.2: whether attack-move was armed when the right button went
    /// down (the minimap right-click honors it as an attack-move order; a
    /// world right-click disarms and issues the context order).
    pub(crate) right_press_armed: bool,
    /// M10.2 (PLAN §1.2): the middle button's last cursor position while
    /// held — horizontal travel rotates the camera (Generals-style
    /// middle-drag rotate; it panned before this pass).
    pub(crate) middle_last: Option<(f64, f64)>,
    /// M10.2 (PLAN §1.3): when the last left *click* (not drag) landed —
    /// the double-click window's clock for same-kind select-all.
    pub(crate) last_left_click: Option<Instant>,
    /// M10.2 (PLAN §1.3): the last control-group digit press and when it
    /// landed — a double tap recalls the group and centers the camera on it.
    pub(crate) last_digit_press: Option<(usize, Instant)>,
    /// PLAN-M11 §2 (phase 2): the E key's double-tap clock — two presses
    /// inside DOUBLE_TAP_MS widen the same-kind select to the whole map.
    pub(crate) last_e_press: Option<Instant>,
    /// PLAN-M11 §2 (phase 2): the Q key's double-tap clock (same window).
    pub(crate) last_q_press: Option<Instant>,
    /// PLAN-M11 §2 (phase 2): the kind of the most recently *clicked*
    /// entity — E's fallback kind for an empty or mixed selection. `None`
    /// until the first pick (E is inert then; the card says so).
    pub(crate) last_clicked_kind: Option<KindId>,
    /// PLAN-M11 §2 (phase 2): the content's combat kinds (an Attack
    /// capability), computed once per match — Q's filter. Never per frame.
    pub(crate) combat_kinds: BTreeSet<KindId>,
    /// PLAN-M11 §2 (phase 2): the content's unit kinds (a Move
    /// capability — anything without it is a structure or a node),
    /// computed once per match. Ctrl+Left/Right cycles these.
    pub(crate) unit_kinds: BTreeSet<KindId>,
    /// PLAN-M11 §2 (phase 2): the content's command-center kind (the
    /// engine plan's own rule — the first producer whose producibles
    /// include no attacking kind). H's target; `None` when the faction
    /// has none (H falls back to the base anchor).
    pub(crate) command_center_kind: Option<KindId>,
    /// M10.2 (PLAN §1.3): the last death's ground position (the Space
    /// jump's preferred target, else the start anchor — "jump to the last
    /// event or base").
    pub(crate) last_event_ground: Option<(f32, f32)>,
    /// M10.2 (PLAN §1.3): whether the window cursor is currently the
    /// crosshair (the armed attack-move state's cursor; set on change only).
    pub(crate) crosshair_cursor: bool,
    /// M10.2 (PLAN §1.2): whether edge scrolling is on (the settings toggle
    /// itself is Phase 3; the flag is wired now so the toggle only flips a
    /// bool then). The band width and depth scaling live in `input.rs`.
    pub(crate) edge_scroll_enabled: bool,
    /// M9.1: whether the window has keyboard focus (edge scrolling runs
    /// focused only, so an unfocused game never steals the desktop).
    pub(crate) focused: bool,
    /// M9.1 (plan §11.3): 'A' arms attack-move; the next left-click places
    /// it. Firing on key-down fought the key's camera-pan role and spammed
    /// orders through key repeats (the DEBT-008 human pass: "the control
    /// is bad").
    pub(crate) attack_move_armed: bool,
    /// M9.1: the last submitted order (its seq and the ground point it
    /// targeted) — rejection feedback pings there.
    pub(crate) last_order: Option<(u32, Vec2Fx)>,
    /// M9.1: the human start anchor (the opening camera focus and restart's
    /// re-focus), from the map's declared starts.
    pub(crate) start_anchor: (f32, f32),
    /// M9.1: the content's worker kind id (right-click gather resolution).
    pub(crate) worker_kind: Option<KindId>,
    /// M9.1: the content's gatherable node kind id (right-click gather
    /// resolution).
    pub(crate) node_kind: Option<KindId>,
    pub(crate) last_frame: Instant,
    pub(crate) command_seq: u32,
    /// Display names of the loaded resources, indexed by ResourceId (the
    /// HUD's labels — data, not hardcoded).
    pub(crate) resource_names: Vec<String>,
    /// Whether the §11.6 debug overlay is visible (F10).
    pub(crate) debug_overlay: bool,
    /// Verification affordance: exit after this many presented frames.
    pub(crate) frames_budget: Option<u64>,
    /// How many frames have been presented (the `--frames` counter).
    pub(crate) frames_presented: u64,
    /// How many commands the client has submitted this run (the windowed
    /// smoke summary's evidence that input reached the simulation).
    pub(crate) commands_submitted: u32,
    /// How many matches this session started (the summary's honesty about
    /// a menu-end state: "no match" and "a match that was dropped" are
    /// different facts).
    pub(crate) matches_started: u32,
    /// M10.2 Phase 4 (PLAN §4, the audio pass): the sink that voices the
    /// nine cues — rodio behind the engine's `AudioSink` seam when the
    /// startup probe found a device, the null counting arm when it did
    /// not (ADR-0002; FD-9: fed after the step from the presentation
    /// layer, feeds nothing back).
    pub(crate) audio: sound::Sound,
    /// M9 (plan §11.2/§11.5, the feel pass): event-driven feedback — hit
    /// flashes, command acknowledgment pings, and the counters the smoke
    /// summary reports.
    pub(crate) feedback: FeedbackState,
    /// The client's own interpolator over the *fog-filtered* view: the human
    /// sees what the human's player can see (FD-8 from the rendering side).
    /// The host's own interpolator keeps blending full snapshots for anyone
    /// who needs them; everything drawn here comes from this one.
    pub(crate) view_interp: Interpolator,
    /// The sim tick the fog texture was last refreshed at (fog updates once
    /// per sim step, not once per presented frame). `u64::MAX` forces the
    /// first refresh.
    pub(crate) fog_tick: u64,
    /// The last fog alpha bytes (row-major, one per tile) — compared against
    /// the incoming view so a new texture upload only happens on change.
    pub(crate) fog_bytes: Vec<u8>,
    /// Every visible entity's last-seen world position, kind, and owner —
    /// the data a death fade needs at the moment a `Died` event arrives
    /// (the event carries only the id).
    pub(crate) last_seen: BTreeMap<EntityId, (glam::Vec3, KindId, PlayerId)>,
    /// The player's own view as of this frame — the command card's data
    /// source (queues, own entities for requirement checks, the ledger).
    pub(crate) latest_view: Option<PlayerView>,
    /// Active structure placement (§11.3's build placement mode): the
    /// worker that will build, the structure, and the tile the ghost
    /// currently hovers (None until the cursor first hits the ground).
    pub(crate) placement: Option<PlacementState>,
    /// The bottom bar's clickable regions, rebuilt every frame by the UI
    /// pass and hit-tested before world clicks.
    pub(crate) ui_buttons: Vec<Button>,
    /// The active menu screen's clickable regions (PLAN-M10.2 §3.6), rebuilt
    /// every frame by the menu pass — hit-tested before any world input
    /// routing (empty while a match owns the input).
    pub(crate) menu_buttons: Vec<MenuButton>,
    /// M10.1 (plan §6.5): `--record <path>` — when set, the windowed run's
    /// replay is written at match end / clean exit (a file `tools
    /// replay-verify` accepts — A2's evidence channel for the playtest).
    pub(crate) record_path: Option<PathBuf>,
    /// M10.1: whether the current match segment's record was already
    /// written at its end (the exit path writes only when it never was —
    /// restart resets this with the segment).
    pub(crate) record_written: bool,
    /// M10.1 (plan §6.5/§11.6): the checkpoint trail feeding `--record` and
    /// the F11 dump — the tick-0 hash plus every periodic checkpoint the
    /// host reported per frame (the same assembly the tools' AI-match
    /// recorder uses). Reset by every begin_match; empty at the menu.
    pub(crate) checkpoints: Vec<(u32, u64)>,
}

/// Active structure placement (plan §11.3's build placement mode with a
/// legality preview).
pub(crate) struct PlacementState {
    /// The worker that will receive the Build command.
    pub(crate) worker: EntityId,
    /// The structure kind to place.
    pub(crate) structure: KindId,
    /// The tile the ghost currently hovers over.
    pub(crate) ghost_tile: Option<(i32, i32)>,
}

impl App {
    pub(crate) fn new(
        tree: ContentTree,
        default_bundle: ContentBundle,
        seed_prefill: Option<u64>,
        record_path: Option<PathBuf>,
        settings: config::Settings,
        audio: sound::Sound,
        pack: Option<AssetPack>,
    ) -> anyhow::Result<Self> {
        let resource_names: Vec<String> = tree
            .rules
            .resources
            .iter()
            .map(|resource| resource.display_name.clone())
            .collect();
        // M9.1's opening view (now the menu's backdrop): the default map's
        // player start at the readable opening distance. A started match
        // re-frames on its own map's anchor (begin_match).
        let start_anchor = default_bundle
            .map
            .starts
            .iter()
            .find(|start| start.player == 0)
            .map(|start| (start.x as f32 + 0.5, start.y as f32 + 0.5))
            .unwrap_or((
                default_bundle.map.width as f32 / 2.0,
                default_bundle.map.height as f32 / 2.0,
            ));
        let mut camera = RtsCamera::new(
            default_bundle.map.width,
            default_bundle.map.height,
            16.0 / 9.0,
        );
        camera.focus(start_anchor.0, start_anchor.1, START_CAMERA_DISTANCE);
        // PLAN-M10.2 §4.1/§4.3's "probe once at startup" now happens in the
        // WIRING layer (main / the headless smoke) — A-002 Part 1: the App
        // receives its sink instead of building one, so a unit test can
        // construct a full App over `Sound::without_device` without ever
        // touching the machine's audio hardware. The product behavior is
        // unchanged: main() probes exactly once before the first frame.
        Ok(Self {
            tree,
            screen: AppState::at_main_menu(),
            seed_prefill,
            settings,
            pack,
            bundle: default_bundle,
            host: None,
            match_mode: MatchMode::PlayerVsAi,
            setup: MatchMode::PlayerVsAi.match_setup(seed_prefill.unwrap_or(7)),
            map_index: 0,
            mission_index: None,
            mission_hud: None,
            cutscene: None,
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
            last_e_press: None,
            last_q_press: None,
            last_clicked_kind: None,
            combat_kinds: BTreeSet::new(),
            unit_kinds: BTreeSet::new(),
            command_center_kind: None,
            last_event_ground: None,
            crosshair_cursor: false,
            // DEBT-014 repaid (PLAN-M10.2 §3.4): the toggle now comes from
            // the settings file (true by default) and the settings screen
            // flips it live.
            edge_scroll_enabled: settings.edge_scroll,
            focused: true,
            attack_move_armed: false,
            last_order: None,
            start_anchor,
            worker_kind: None,
            node_kind: None,
            last_frame: Instant::now(),
            command_seq: 0,
            matches_started: 0,
            resource_names,
            debug_overlay: false,
            frames_budget: None,
            frames_presented: 0,
            commands_submitted: 0,
            audio,
            feedback: FeedbackState::default(),
            view_interp: Interpolator::new(),
            fog_tick: u64::MAX,
            fog_bytes: Vec::new(),
            last_seen: BTreeMap::new(),
            latest_view: None,
            placement: None,
            ui_buttons: Vec::new(),
            menu_buttons: Vec::new(),
            record_path,
            record_written: false,
            checkpoints: Vec::new(),
        })
    }

    /// A-002 Part 2.2: the F10 overlay's first line — the build identity plus
    /// the CURRENT bundle's content hash (a cached u64 read; a started match
    /// on another map shows that map's bundle, so the overlay never lies
    /// about which content is running).
    pub(crate) fn version_line(&self) -> String {
        format!(
            "pandemonium {} git {}  content {:#018x}",
            env!("CARGO_PKG_VERSION"),
            git_sha(),
            self.bundle.content_hash()
        )
    }

    /// Builds a fresh `MatchHost` for a mode (M8 + PLAN-M10.2 §3.3, A-112):
    /// one scripted Alpha controller per `mode.ai_slots()` — the opponent in
    /// Player vs AI, both players in spectate, none in Sandbox. The
    /// controller's RNG seed derives from the match seed (plan §9.6: seeded,
    /// never its own entropy), so a same-seed restart reproduces the same
    /// match.
    pub(crate) fn build_host(
        bundle: &ContentBundle,
        setup: MatchSetup,
        mode: MatchMode,
    ) -> MatchHost {
        let world = bundle.world();
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = mode
            .ai_slots()
            .into_iter()
            .map(|slot| {
                (
                    slot,
                    Box::new(alpha_controller(bundle, &world, slot, setup.seed, None))
                        as Box<dyn Controller>,
                )
            })
            .collect();
        MatchHost::with_controllers(&world, setup, controllers)
    }

    /// Builds a mission match's host (A-011 Task 1, over Task 1.5's engine
    /// seam): the opponent slot's scripted Alpha controller — the default
    /// plan is the slice's opponent (ADR-0201 §9: "AI: nothing") — plus the
    /// mission's resolved objective set through [`MatchHost::with_mission`],
    /// so the sim evaluates the mission's objectives and emits
    /// `Event::ObjectiveState`. The construction path is the exact
    /// `with_controllers` body with the sim built mission-carrying: tick-0
    /// state is identical by construction.
    pub(crate) fn build_mission_host(
        bundle: &ContentBundle,
        setup: MatchSetup,
        mission: MissionData,
    ) -> MatchHost {
        let world = bundle.world();
        // B-021: the mission tunes the scripted opponent — its ai_settings
        // ride the controller's plan (None fields keep the script's consts;
        // a mission without the block tunes nothing, byte for byte). The
        // objectives stay the simulation's (FD-7: the AI never sees them).
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = [AI]
            .into_iter()
            .map(|slot| {
                (
                    slot,
                    Box::new(alpha_controller(
                        bundle,
                        &world,
                        slot,
                        setup.seed,
                        Some(&mission),
                    )) as Box<dyn Controller>,
                )
            })
            .collect();
        MatchHost::with_mission(&world, setup, controllers, mission)
    }

    /// The mission identity a mission match's replay record carries
    /// (ADR-0201 §7, the audit's AUD-002): the domain-separated id of the
    /// mission `begin_mission` resolved — `None` for a plain match. Pure
    /// over `(tree, mission_index)`; both recorders (`write_recording`,
    /// `dump_bug_report`) stamp it, so a mission record names its mission
    /// for `tools replay-verify`'s third comparison and a plain record
    /// stays mission-less. An out-of-range index is `None` (defensive —
    /// `mission_index` is only ever set from a valid index, and the tree
    /// never changes after load).
    pub(crate) fn mission_replay_id(
        tree: &ContentTree,
        mission_index: Option<usize>,
    ) -> Option<u64> {
        let index = mission_index?;
        let def = tree.missions.get(index)?;
        Some(tree.mission_id(&def.id))
    }

    /// Starts a mission match (A-011 Task 1, ADR-0201 Phase 1): bundles the
    /// mission's map, resolves its objectives through the content seam
    /// (`ContentTree::mission_data` — the mission player is the first
    /// map start's player, slot 0 on this content, the human), and builds
    /// the host through the engine mission seam. The setup is the Player vs
    /// AI shape (the human runs the mission player's slot; the scripted
    /// Alpha AI runs the opponent), so the match plays through the exact
    /// existing order/host paths — only the mission rides along. A failure
    /// to bundle or resolve is surfaced and keeps the menu (never crash the
    /// loop — the same discipline as `begin_match`).
    pub(crate) fn begin_mission(&mut self, mission_index: usize, seed: u64) {
        let Some(def) = self.tree.missions.get(mission_index).cloned() else {
            println!(
                "pandemonium client — mission index {mission_index} is out of range \
                 ({} mission(s) loaded)",
                self.tree.missions.len()
            );
            return;
        };
        // B-019: the mission-aware bundle path — the mission's own map
        // bundle with its faction_overrides resolved, so the world (roster,
        // production, starting forces) and the scripted opponent's plan
        // (derived from the faction's production lists) both see the
        // EFFECTIVE faction the mission fields. A mission without overrides
        // resolves the base faction — the plain bundle, exactly as before.
        let bundle = match self.tree.bundle_for_mission(&def.id) {
            Ok(bundle) => bundle,
            Err(error) => {
                println!(
                    "pandemonium client — could not bundle map '{}' for mission '{}': {error:#}",
                    def.map, def.id
                );
                return;
            }
        };
        // The mission player is the human's slot (the first map start's
        // player — slot 0 on the Alpha content; the content seam's Phase 1
        // law). The objectives resolve to KindIds through the tree's entity
        // order — the exact ids the bundled world's entities carry.
        let mission = match self.tree.mission_data(&def.id, HUMAN) {
            Ok(mission) => mission,
            Err(error) => {
                println!(
                    "pandemonium client — could not resolve objectives for mission '{}': {error:#}",
                    def.id
                );
                return;
            }
        };
        // A-011: the HUD's objective lines derive from the same resolved
        // objective set the simulation receives (built here, before the set
        // moves into the host) — the line indices are the `Event::ObjectiveState`
        // indices by construction.
        let lines = ui::objective_lines(&mission, &bundle);
        let setup = MatchMode::PlayerVsAi.match_setup(seed);
        // M9.1: the right-click context resolver needs the content's worker
        // and node kind ids — a mission match plays the same order paths.
        // (Kind resolution only: no tuning is read here — the opponent's
        // controller, built in build_mission_host above, carries the
        // mission's ai_settings.)
        let world = bundle.world();
        let plan = alpha_plan(&bundle, &world, HUMAN, setup.seed, None);
        let host = Self::build_mission_host(&bundle, setup.clone(), mission);
        self.matches_started += 1;
        self.bundle = bundle;
        self.camera
            .set_bounds(self.bundle.map.width, self.bundle.map.height);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_terrain(&terrain_mesh(&self.bundle.map, crate::HEIGHT_SCALE));
        }
        self.match_mode = MatchMode::PlayerVsAi;
        self.setup = setup;
        self.map_index = self
            .tree
            .maps
            .iter()
            .position(|map| map.id == def.map)
            .unwrap_or(0);
        self.host = Some(host);
        // The per-match derived state (restart's source of truth).
        self.start_anchor = self
            .bundle
            .map
            .starts
            .iter()
            .find(|start| start.player == 0)
            .map(|start| (start.x as f32 + 0.5, start.y as f32 + 0.5))
            .unwrap_or((
                self.bundle.map.width as f32 / 2.0,
                self.bundle.map.height as f32 / 2.0,
            ));
        self.worker_kind = Some(plan.worker);
        self.node_kind = plan.node;
        // PLAN-M11 §2 (phase 2): the per-match selection vocabulary — Q's
        // combat kinds and H's command-center kind, both derived from the
        // bundle once (never per frame).
        self.combat_kinds = combat_kinds(&self.bundle);
        self.unit_kinds = unit_kinds(&self.bundle);
        self.command_center_kind = command_center_kind(&self.bundle);
        self.reset_match_state();
        // The mission-scoped state: the restart source (the tree index) and
        // the presentation tracker (the HUD lines and end-screen text read
        // the same objective states — built from the resolved objective set,
        // so the indices are the event indices by construction).
        self.mission_index = Some(mission_index);
        self.mission_hud = Some(ui::MissionHud::new(def.display_name.clone(), lines));
        // PLAN-M10.2 §3.4: the debug overlay's default at match start, and
        // the user zoom limits re-clamp the opening view.
        self.debug_overlay = self.settings.debug_overlay;
        self.camera
            .clamp_distance_to(self.settings.zoom_min, self.settings.zoom_max);
        // M10.1: the checkpoint trail opens at the fresh match's tick-0
        // hash — identical to the mission-less construction by the seam's
        // shared `Sim::build` body (pinned by the engine seam test).
        self.checkpoints = vec![(0, self.host.as_ref().expect("just built").state_hash())];
        self.record_written = false;
        // A-011 (ADR-0201 §5, Task 3): the mission-start cutscene — pause
        // the host, pan the camera from the enemy's start to the player's
        // start over the frozen live world, caption the mission's name and
        // objective summary. Any key ends the scene and unpauses.
        let enemy_anchor = self
            .bundle
            .map
            .starts
            .iter()
            .find(|start| start.player == 1)
            .map(|start| (start.x as f32 + 0.5, start.y as f32 + 0.5))
            .unwrap_or((
                self.bundle.map.width as f32 / 2.0,
                self.bundle.map.height as f32 / 2.0,
            ));
        let intro = self.mission_hud.as_ref().expect("just built").intro_line();
        self.cutscene = Some(crate::cutscene::Cutscene::mission_start(
            &def.display_name,
            &intro,
            enemy_anchor,
            self.start_anchor,
        ));
        if let Some(host) = &mut self.host {
            host.set_paused(true);
        }
        self.screen = AppState {
            screen: Screen::InMatch,
            focus: 0,
        };
    }

    /// Ends the active cutscene, if any (ADR-0201 §5): the camera snaps to
    /// the scene's resting focus (the player's start at the opening
    /// distance — a readable view whatever moment the skip landed on) and
    /// the host unpauses. The host was paused by the scene's start, so a
    /// mission-less call is a no-op.
    pub(crate) fn end_cutscene(&mut self) {
        let Some(mut scene) = self.cutscene.take() else {
            return;
        };
        // The player's own skip edge (§5 (iii)): the scene is over from
        // here on, whatever moment it ended at.
        scene.skip();
        let (x, z, distance) = scene.resting_focus();
        self.camera.focus(x, z, distance);
        if let Some(host) = &mut self.host {
            host.set_paused(false);
        }
    }

    /// One input event during an active cutscene (ADR-0201 §5 (iii): every
    /// cutscene is skippable at any moment — any key, Esc included, ends
    /// the scene and unpauses). The wiring layer's single call site for the
    /// skip path; a no-op when no scene runs.
    pub(crate) fn cutscene_input(&mut self) {
        self.end_cutscene();
    }

    /// Starts a match (PLAN-M10.2 §3.3): bundles the selected map, builds the
    /// mode's setup and host, and resets every match-scoped state — the same
    /// drop + reconstruct discipline M8's restart established (A15), so
    /// determinism is unchanged: the same (mode, seed, map) builds the same
    /// match bit-for-bit.
    pub(crate) fn begin_match(&mut self, mode: MatchMode, seed: u64, map_index: usize) {
        let map_id = self
            .tree
            .maps
            .get(map_index)
            .map(|map| map.id.clone())
            .unwrap_or_default();
        let bundle = match self.tree.bundle(&map_id) {
            Ok(bundle) => bundle,
            Err(error) => {
                // The map came from the same tree's list; a failure here is
                // not a state the loop can render. Surface and keep the
                // menu (never crash the loop — PLAN §4.3's spirit).
                println!("pandemonium client — could not bundle map '{map_id}': {error:#}");
                return;
            }
        };
        let setup = mode.match_setup(seed);
        // M9.1: the right-click context resolver needs the content's worker
        // and node kind ids — the engine's alpha-plan resolution (capability
        // shaped, never name-matched) already derives exactly those.
        let world = bundle.world();
        let plan = alpha_plan(&bundle, &world, HUMAN, setup.seed, None);
        let host = Self::build_host(&bundle, setup.clone(), mode);
        self.matches_started += 1;
        self.bundle = bundle;
        self.camera
            .set_bounds(self.bundle.map.width, self.bundle.map.height);
        if let Some(renderer) = &mut self.renderer {
            renderer.set_terrain(&terrain_mesh(&self.bundle.map, crate::HEIGHT_SCALE));
        }
        self.match_mode = mode;
        self.setup = setup;
        self.map_index = map_index;
        self.host = Some(host);
        // The per-match derived state (restart's source of truth).
        self.start_anchor = self
            .bundle
            .map
            .starts
            .iter()
            .find(|start| start.player == 0)
            .map(|start| (start.x as f32 + 0.5, start.y as f32 + 0.5))
            .unwrap_or((
                self.bundle.map.width as f32 / 2.0,
                self.bundle.map.height as f32 / 2.0,
            ));
        self.worker_kind = Some(plan.worker);
        self.node_kind = plan.node;
        // PLAN-M11 §2 (phase 2): the per-match selection vocabulary — Q's
        // combat kinds and H's command-center kind, both derived from the
        // bundle once (never per frame).
        self.combat_kinds = combat_kinds(&self.bundle);
        self.unit_kinds = unit_kinds(&self.bundle);
        self.command_center_kind = command_center_kind(&self.bundle);
        self.reset_match_state();
        // PLAN-M10.2 §3.4: the debug overlay's default at match start (F10
        // stays the live toggle), and the user zoom limits re-clamp the
        // opening view.
        self.debug_overlay = self.settings.debug_overlay;
        self.camera
            .clamp_distance_to(self.settings.zoom_min, self.settings.zoom_max);
        // M10.1: the checkpoint trail opens at the fresh match's tick-0
        // hash (the same first entry the tools' recorder pushes).
        self.checkpoints = vec![(0, self.host.as_ref().expect("just built").state_hash())];
        self.record_written = false;
        self.screen = AppState {
            screen: Screen::InMatch,
            focus: 0,
        };
    }

    /// Restarts the live match (M8, plan §9.7, A15): drop + reconstruct
    /// through the same (mode, seed, map) record Start used — a same-seed
    /// restart reproduces the same match bit-for-bit (test-pinned; do not
    /// weaken).
    pub(crate) fn restart_match(&mut self) {
        // A-011: a mission match restarts as the same mission — the same
        // (mission, seed) record Start was pressed with (the A15 shape over
        // the mission dimension).
        if let Some(mission_index) = self.mission_index {
            let seed = self.setup.seed;
            self.begin_mission(mission_index, seed);
            return;
        }
        let (mode, seed, map_index) = (self.match_mode, self.setup.seed, self.map_index);
        self.begin_match(mode, seed, map_index);
    }

    /// Drops the match and returns to the main menu (PLAN-M10.2 §3.1's
    /// `-> MainMenu` edges). A menu-only session must never panic, write a
    /// report, or submit a command — everything match-scoped clears here.
    pub(crate) fn drop_match(&mut self) {
        self.host = None;
        self.reset_match_state();
        self.checkpoints.clear();
        self.record_written = false;
        // The menu backdrop returns to the default map; keep its terrain,
        // camera bounds, and framing in sync after a larger map was played.
        if let Some(default_map) = self.tree.maps.first().cloned() {
            if let Ok(bundle) = self.tree.bundle(&default_map.id) {
                self.bundle = bundle;
            }
            self.map_index = 0;
            self.start_anchor = default_map
                .starts
                .iter()
                .find(|start| start.player == 0)
                .map(|start| (start.x as f32 + 0.5, start.y as f32 + 0.5))
                .unwrap_or((
                    default_map.width as f32 / 2.0,
                    default_map.height as f32 / 2.0,
                ));
            self.camera
                .set_bounds(default_map.width, default_map.height);
            self.camera.focus(
                self.start_anchor.0,
                self.start_anchor.1,
                START_CAMERA_DISTANCE,
            );
            if let Some(renderer) = &mut self.renderer {
                renderer.set_terrain(&terrain_mesh(&default_map, crate::HEIGHT_SCALE));
            }
        }
        self.screen = AppState::at_main_menu();
    }

    /// The shared reset every match boundary runs (begin and drop): the
    /// camera re-frames on the base, the input-layer state machines go
    /// idle, and the counters zero.
    pub(crate) fn reset_match_state(&mut self) {
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
        self.last_e_press = None;
        self.last_q_press = None;
        self.last_clicked_kind = None;
        self.last_event_ground = None;
        self.crosshair_cursor = false;
        self.attack_move_armed = false;
        self.last_order = None;
        self.drag_start = None;
        // A-011 (ADR-0201 Phase 1): the mission-scoped state resets with the
        // match — the restart source and the HUD/end-screen tracker.
        // `begin_mission` re-populates them after this reset; a mission-less
        // start stays clean.
        self.mission_index = None;
        self.mission_hud = None;
        self.cutscene = None;
        // M9.1: the camera re-frames on the base and the armed order clears
        // — a fresh match starts from the same readable opening view.
        self.camera.focus(
            self.start_anchor.0,
            self.start_anchor.1,
            START_CAMERA_DISTANCE,
        );
        // M9: the feel pass's state resets with the match — a fresh match
        // starts with clean feedback (no stale flashes or pings). The
        // Phase 4 sink resets its counters and rate-limit windows with it;
        // the device, the cue bank, and the volume persist (the audio
        // outlives matches — a fresh match is not a fresh probe).
        self.audio.reset();
        self.feedback = FeedbackState::default();
    }

    /// The fog-filtered interpolated snapshot for this display frame: the
    /// same blend point the host's full-snapshot path uses, over the entity
    /// set the human's player may see. Every drawn pixel and every screen
    /// query (selection, context orders) reads through this.
    pub(crate) fn view_snapshot(&self) -> pandemonium_engine::RenderSnapshot {
        match (&self.view_interp, &self.host) {
            (interp, Some(host)) => interp.render(host.alpha()),
            (_, None) => pandemonium_engine::RenderSnapshot::default(),
        }
    }

    /// The live match's host (the match path only — every caller runs after
    /// the menu early-return, so the expect is the invariant that a host
    /// exists exactly while the screen is InMatch/PauseMenu/EndScreen).
    pub(crate) fn host(&self) -> &MatchHost {
        self.host.as_ref().expect("a live match")
    }

    /// Interprets one menu effect (PLAN §3.1: "effects are an enum the app
    /// layer interprets").
    pub(crate) fn apply_effect(&mut self, effect: Effect, event_loop: &ActiveEventLoop) {
        match effect {
            Effect::None => {}
            Effect::StartMatch {
                mode,
                seed,
                map_index,
            } => self.begin_match(mode, seed, map_index),
            // A-011 (ADR-0201 Phase 1): the mission picker's Start resolves
            // through the engine mission seam.
            Effect::StartMission {
                mission_index,
                seed,
            } => self.begin_mission(mission_index, seed),
            Effect::RestartMatch => self.restart_match(),
            Effect::DropMatch => self.drop_match(),
            Effect::Pause => {
                if let Some(host) = &mut self.host {
                    host.set_paused(true);
                }
            }
            Effect::Unpause => {
                if let Some(host) = &mut self.host {
                    host.set_paused(false);
                }
            }
            Effect::SaveSettings => self.save_settings(),
            Effect::AdjustSetting { row, dir } => self.adjust_setting(row, dir),
            Effect::QuitApp => event_loop.exit(),
        }
    }

    /// Persists the settings (the settings screen's Done). Best effort: the
    /// run keeps the in-memory values even when the write fails, and a
    /// machine with no config directory (A-109) simply never persists.
    pub(crate) fn save_settings(&mut self) {
        match config::save(&self.settings) {
            Some(Ok(())) => {}
            Some(Err(error)) => {
                println!("pandemonium client — settings save failed: {error}");
            }
            None => {} // persistence disabled (no config dir) — A-109
        }
    }

    /// Adjusts one settings row and applies it live (PLAN-M10.2 §3.4): the
    /// edge-scroll flag flips immediately (DEBT-014's repayment — the flag
    /// has been wired since Phase 1), the zoom limits re-clamp the camera
    /// now, the debug overlay follows (it is also the match-start default),
    /// fullscreen toggles the window, and pan speed is read live from the
    /// struct already. M10.2 Phase 4 (DEBT-011 repaid): master volume now
    /// goes straight to the sink (every later voice uses it) and the new
    /// mute row beats it — A-115's semantics: live now, Done saves, Esc
    /// leaves this run's value unsaved.
    pub(crate) fn adjust_setting(&mut self, row: usize, dir: i32) {
        self.settings = config::adjust_setting(self.settings, row, dir);
        match row {
            0 => self.edge_scroll_enabled = self.settings.edge_scroll,
            2 | 3 => self
                .camera
                .clamp_distance_to(self.settings.zoom_min, self.settings.zoom_max),
            4 => self.audio.set_volume(self.settings.master_volume),
            5 => self.audio.set_muted(self.settings.muted),
            6 => self.debug_overlay = self.settings.debug_overlay,
            7 => self.apply_fullscreen(),
            _ => {}
        }
    }

    /// Applies the fullscreen setting to the window (best effort — a
    /// window manager may refuse; Xvfb has none at all, and the re-test on
    /// a real display is the honest verdict, T9).
    pub(crate) fn apply_fullscreen(&mut self) {
        if let Some(window) = &self.window {
            if self.settings.fullscreen {
                window.set_fullscreen(Some(winit::window::Fullscreen::Borderless(None)));
            } else {
                window.set_fullscreen(None);
            }
        }
    }

    /// PLAN-M11 §2 (phase 2): one AttackHit's radar contribution — the
    /// Space jump's tracked ground becomes the victim's position when the
    /// victim is an own entity (an attack on the player's forces is the
    /// event a player most needs to see; attacks on enemies are not radar
    /// events). The victim's ground position comes from `last_seen` (own
    /// entities are always in the fog-filtered view). Called per event, in
    /// arrival order, so the most recent radar event wins.
    pub(crate) fn note_attack_radar(&mut self, target: EntityId) {
        if let Some(&(pos, _, owner)) = self.last_seen.get(&target) {
            if owner == HUMAN {
                self.last_event_ground = Some((pos.x, pos.z));
            }
        }
    }

    /// M10.1 (plan §6.5): folds one frame outcome's periodic checkpoints
    /// into the trail `--record` and F11 read (the same accumulation the
    /// tools' AI-match recorder performs).
    pub(crate) fn collect_checkpoints(&mut self, outcome: &FrameOutcome) {
        for &(tick, hash) in &outcome.hashes {
            self.checkpoints.push((tick, hash));
        }
    }

    /// M10.1 (plan §6.5): writes the current match segment's replay record
    /// to the `--record` path — a file `tools replay-verify` accepts, built
    /// exactly like the tools' recorder (content hash, map id, the host's
    /// command log, the checkpoint trail, the forced final hash).
    /// PLAN-M10.2 §3.5: a menu-only session records nothing — a run that
    /// never left the menu writes no file; a run whose match starts later
    /// records exactly the same segment a menu-less run of the same match
    /// would.
    pub(crate) fn write_recording(&mut self) {
        let Some(path) = self.record_path.clone() else {
            return;
        };
        let Some(host) = &mut self.host else {
            return; // no live match — nothing to record
        };
        let world = self.bundle.world();
        // AUD-002 (ADR-0201 §7): a mission match's record names its mission
        // — the id of the mission `begin_mission` resolved; a plain match
        // records `None`.
        let mission_id = Self::mission_replay_id(&self.tree, self.mission_index);
        let replay = report::replay_record(
            &world,
            &self.setup,
            host.log(),
            &self.checkpoints,
            host.tick(),
            host.state_hash(),
            mission_id,
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

    /// M10.1 (plan §11.6): the deterministic F11 bug-report dump — the replay
    /// record of the match so far (`pandemonium-report-<seed>-tick<tick>.pdrp`)
    /// plus the sidecar info file (`…-info.txt`), written beside `--record`'s
    /// path or in the working directory, both filenames printed so the tester
    /// can attach them to the report. Works while paused (the dump reads the
    /// current state); deterministic content only, so two dumps at the same
    /// tick produce identical bytes.
    pub(crate) fn dump_bug_report(&mut self) {
        let Some(host) = &self.host else {
            // PLAN-M10.2 §3.5: at the menu there is no match to report —
            // the dump is a match-scoped tool.
            println!(
                "pandemonium bug report: no live match (at the {})",
                self.screen.name()
            );
            return;
        };
        let world = self.bundle.world();
        let tick = host.tick();
        // AUD-002 (ADR-0201 §7): the bug-report dump names the mission too —
        // the reproducer needs the exact match identity, mission included.
        let mission_id = Self::mission_replay_id(&self.tree, self.mission_index);
        let replay = report::replay_record(
            &world,
            &self.setup,
            host.log(),
            &self.checkpoints,
            tick,
            host.state_hash(),
            mission_id,
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
}

impl App {
    /// The windowed smoke summary: what the `--frames` run proved (frames
    /// presented, sim progress, commands submitted through input, selection,
    /// and — the M9 full-loop evidence — whether the match resolved).
    pub(crate) fn windowed_summary(&self) {
        // PLAN-M10.2 §3 (T7's honesty): a `--frames` run that never left the
        // menu reports the menu — no sim commands exist to summarize, and no
        // record is written (the menu submits nothing).
        let Some(host) = &self.host else {
            let seed = match self.seed_prefill {
                Some(seed) => format!("--seed {seed}"),
                None => "random".to_string(),
            };
            let session = if self.matches_started == 0 {
                "no match this run".to_string()
            } else {
                format!(
                    "{} match{} started this run, none live",
                    self.matches_started,
                    if self.matches_started == 1 { "" } else { "es" }
                )
            };
            println!(
                "pandemonium client — windowed smoke: {} frames presented, at the {} ({session}; \
                 seed prefill {seed})",
                self.frames_presented,
                self.screen.name()
            );
            // PLAN-M10.2 §4.5: what the audio did this run — the arm and the
            // cues' fates, the same evidence line the match branch prints.
            println!("pandemonium client — audio: {}", self.audio.evidence_line());
            return;
        };
        let outcome = match host.outcome() {
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
             seed {}, {} commands submitted, selection {}, {}, at the {}",
            self.frames_presented,
            host.tick(),
            host.state_hash(),
            self.setup.seed,
            self.commands_submitted,
            self.selection.len(),
            outcome,
            self.screen.name()
        );
        // PLAN-M10.2 §4.5: what the audio did this match segment — the arm
        // (active or fallback) and the cues' three fates. Under Xvfb this
        // always reads the null fallback (the machine cannot hear; the
        // counters are the claim, never "audio ok").
        println!("pandemonium client — audio: {}", self.audio.evidence_line());
    }

    /// Menu pointer hover (PLAN-M10.2 §3.6): the hit-test against the menu
    /// buttons moves the machine's focus — mouse and keyboard share the one
    /// navigation core. (The button rects arrive with the menu pass's commit;
    /// until then there is nothing to hover.)
    pub(crate) fn menu_hover(&mut self) {
        let Some((mx, my)) = self.drag_current else {
            return;
        };
        if let Some(index) = self
            .menu_buttons
            .iter()
            .position(|button| button.contains(mx as f32, my as f32))
        {
            self.screen.focus_index(index);
        }
    }

    /// M10.2 Phase 4 (PLAN §4.2, A-121): activates a menu row — the shared
    /// core of the click path, the Enter path, and the end screen's R
    /// shortcut, wrapped so the UI click cue fires on every activation of
    /// the screens state machine (thin wiring over the pure machine; the
    /// machine itself never owns a sink).
    pub(crate) fn menu_activate(&mut self, index: usize, ctx: &KeyContext) -> (AppState, Effect) {
        self.audio.on_cue(AudioCue::UiClick);
        self.screen.activate_index(index, ctx)
    }

    /// Menu click: activate the hovered/clicked button (the mouse path over
    /// the machine's `activate_index`).
    pub(crate) fn menu_click(&mut self, event_loop: &ActiveEventLoop) {
        let Some((mx, my)) = self.drag_current else {
            return;
        };
        let Some(index) = self
            .menu_buttons
            .iter()
            .position(|button| button.contains(mx as f32, my as f32))
        else {
            return; // a click off every button: nothing (menus never order)
        };
        let (next, effect) = self.menu_activate(index, &self.menu_ctx());
        self.screen = next;
        self.apply_effect(effect, event_loop);
    }

    /// One menu key press (the winit key -> NavKey mapping, then the pure
    /// machine).
    pub(crate) fn menu_key(
        &mut self,
        code: winit::keyboard::KeyCode,
        event_loop: &ActiveEventLoop,
    ) {
        self.keys.clear(); // menus own the keyboard: no ghost camera keys
                           // The end screen keeps the card's R shortcut (rematch).
        if matches!(self.screen.screen, Screen::EndScreen) && code == winit::keyboard::KeyCode::KeyR
        {
            let (next, effect) = self.menu_activate(0, &self.menu_ctx());
            self.screen = next;
            self.apply_effect(effect, event_loop);
            return;
        }
        let key = match code {
            winit::keyboard::KeyCode::ArrowUp => NavKey::Up,
            winit::keyboard::KeyCode::ArrowDown => NavKey::Down,
            winit::keyboard::KeyCode::ArrowLeft => NavKey::Left,
            winit::keyboard::KeyCode::ArrowRight => NavKey::Right,
            winit::keyboard::KeyCode::Enter | winit::keyboard::KeyCode::NumpadEnter => {
                NavKey::Enter
            }
            winit::keyboard::KeyCode::Escape => NavKey::Escape,
            winit::keyboard::KeyCode::Backspace => NavKey::Backspace,
            winit::keyboard::KeyCode::Digit0 | winit::keyboard::KeyCode::Numpad0 => {
                NavKey::Digit('0')
            }
            winit::keyboard::KeyCode::Digit1 | winit::keyboard::KeyCode::Numpad1 => {
                NavKey::Digit('1')
            }
            winit::keyboard::KeyCode::Digit2 | winit::keyboard::KeyCode::Numpad2 => {
                NavKey::Digit('2')
            }
            winit::keyboard::KeyCode::Digit3 | winit::keyboard::KeyCode::Numpad3 => {
                NavKey::Digit('3')
            }
            winit::keyboard::KeyCode::Digit4 | winit::keyboard::KeyCode::Numpad4 => {
                NavKey::Digit('4')
            }
            winit::keyboard::KeyCode::Digit5 | winit::keyboard::KeyCode::Numpad5 => {
                NavKey::Digit('5')
            }
            winit::keyboard::KeyCode::Digit6 | winit::keyboard::KeyCode::Numpad6 => {
                NavKey::Digit('6')
            }
            winit::keyboard::KeyCode::Digit7 | winit::keyboard::KeyCode::Numpad7 => {
                NavKey::Digit('7')
            }
            winit::keyboard::KeyCode::Digit8 | winit::keyboard::KeyCode::Numpad8 => {
                NavKey::Digit('8')
            }
            winit::keyboard::KeyCode::Digit9 | winit::keyboard::KeyCode::Numpad9 => {
                NavKey::Digit('9')
            }
            _ => return, // every other key is inert at a menu
        };
        // PLAN §4.2 (A-121): Enter activates the focused row — routed
        // through the same activation core the mouse path uses, so the UI
        // click cue fires on both (Left/Right adjust values; they are not
        // activations and do not click).
        if matches!(key, NavKey::Enter) {
            let (next, effect) = self.menu_activate(self.screen.focus, &self.menu_ctx());
            self.screen = next;
            self.apply_effect(effect, event_loop);
            return;
        }
        let (next, effect) = self.screen.on_key(key, &self.menu_ctx());
        self.screen = next;
        self.apply_effect(effect, event_loop);
    }

    /// The per-event context for the pure machine (A-111: the random seed is
    /// an input, freshly drawn per event; the seed prefill and the map count
    /// are the app's own field defaults for a fresh New Match screen).
    pub(crate) fn menu_ctx(&self) -> KeyContext {
        KeyContext {
            random_seed: random_seed(),
            seed_prefill: self.seed_prefill,
            map_count: self.tree.maps.len(),
            mission_count: self.tree.missions.len(),
        }
    }

    /// The menu frame (PLAN-M10.2 §3.2/§3.6): the menu pass over a dimmed,
    /// matchless world — the same overlay pass, no host, no HUD, nothing
    /// that could submit or record. The built buttons become this frame's
    /// hit rects (hover/click routing reads them).
    pub(crate) fn draw_menu_frame(&mut self) {
        let maps = self.map_list(); // before the renderer borrow
                                    // A-011: the mission list feeds the picker's Mission field (the map
                                    // list's shape over the tree's missions).
        let missions = self.mission_list();
        // A-009 task 4: the build-identity footer draws on the main menu
        // (the menu-only path lands here). Composed before the renderer
        // borrow so the borrow checker is happy.
        let version_line = self.version_line();
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let viewport = self
            .window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width as f32, size.height as f32)
            })
            .unwrap_or((1920.0, 1080.0));
        // A-009 task 1: the Controls screen renders the standing binding
        // table (report.rs) as informational rows. Every other menu screen
        // passes an empty slice — its rows ARE the buttons.
        let info_lines: &[(&'static str, &'static str)] = match &self.screen.screen {
            Screen::Controls { .. } => report::CONTROLS_TABLE,
            _ => &[],
        };
        let built = ui::build_menu(ui::MenuInput {
            atlas: renderer.atlas(),
            viewport,
            screen: &self.screen,
            settings: &self.settings,
            maps: &maps,
            missions: &missions,
            end_lines: None, // no host behind a menu-only frame
            backdrop: [0.02, 0.03, 0.05, 0.94],
            info_lines,
            version: &version_line,
        });
        self.menu_buttons = built.buttons;
        renderer.queue_ui(&built.quads);
        let snapshot = pandemonium_engine::RenderSnapshot::default();
        let hud = HudState::default();
        renderer.render(Frame {
            snapshot: &snapshot,
            view_projection: self.camera.view_projection(),
            eye: self.camera.eye(),
            selection: &[],
            hud: &hud,
            flashes: &[],
        });
    }

    /// The map list (id, display name) in tree order — the New Match
    /// screen's Map field and the pass's labels.
    pub(crate) fn map_list(&self) -> Vec<(String, String)> {
        self.tree
            .maps
            .iter()
            .map(|map| (map.id.clone(), map.display_name.clone()))
            .collect()
    }

    /// The mission list (id, display name) in tree order — A-011: the
    /// mission picker's Mission field and the pass's labels (the map list's
    /// exact shape over the tree's missions).
    pub(crate) fn mission_list(&self) -> Vec<(String, String)> {
        self.tree
            .missions
            .iter()
            .map(|mission| (mission.id.clone(), mission.display_name.clone()))
            .collect()
    }

    pub(crate) fn draw(&mut self) {
        let dt = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        // A-011 (ADR-0201 §5): an active cutscene drives the camera over the
        // frozen live world (the host is paused, so `advance` below steps
        // nothing); a scene that ran its duration ends and unpauses through
        // the same path a skip takes.
        if let Some(scene) = &mut self.cutscene {
            if scene.advance(dt.as_secs_f32()) == crate::cutscene::SceneProgress::Finished {
                self.end_cutscene();
            }
        }
        if let Some(scene) = &self.cutscene {
            let (x, z, distance) = scene.camera_focus();
            self.camera.focus(x, z, distance);
        }
        // PLAN-M10.2 §3.1: the match resolved -> the state machine promotes
        // InMatch to the end screen (the panel's buttons own the choice
        // now). The live line is the long-run evidence the M9 windowed
        // pass printed only at exit.
        if self.host.as_ref().is_some_and(|h| h.is_finished())
            && matches!(self.screen.screen, Screen::InMatch)
        {
            if let Some(outcome) = self.host.as_ref().and_then(|h| h.outcome()) {
                let winner = match outcome.winner {
                    PlayerId(id) => format!("player {id}"),
                };
                println!(
                    "pandemonium client — match ended, {winner} wins (tick {}, seed {})",
                    outcome.ended_tick, self.setup.seed
                );
            }
            self.screen = self.screen.promote_to_end();
        }
        // PLAN-M10.2 §3.2: at the menu there is no match to advance — the
        // frame is the menu over the (empty) world.
        let outcome = match self.host.as_mut() {
            Some(host) => host.advance(dt),
            None => {
                self.draw_menu_frame();
                return;
            }
        };
        // M10.1 (plan §6.5/§11.6): the trail feeding --record and F11.
        self.collect_checkpoints(&outcome);
        // M10.1 (plan §6.5): --record writes the segment's replay at the
        // match's end — the log is complete there, and the checkpoint at the
        // end tick is the natural replay finale.
        if self.record_path.is_some()
            && !self.record_written
            && self.host.as_ref().is_some_and(|h| h.is_finished())
        {
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
                        // M10.2 (PLAN §1.3) + PLAN-M11 §2: the most recent
                        // death is a radar event for the Space jump (own
                        // and visible enemy deaths, as before).
                        self.last_event_ground = Some((pos.x, pos.z));
                        self.feedback.death(pos, kind, owner, self.frames_presented);
                    }
                }
                // PLAN-M11 §2 (phase 2): an attack landed on an own entity
                // is a radar event too — the same tracked ground the deaths
                // feed, in arrival order (chronological, last wins).
                Event::AttackHit { target, .. } => {
                    self.note_attack_radar(*target);
                }
                // A-011 (ADR-0201 §2, FD-9): the mission HUD's tracker
                // consumes the objective's resolution after the step, from
                // the step's event list — presentation state, never sim
                // state, and never a poll of sim internals.
                Event::ObjectiveState { index, state } => {
                    if let Some(mission) = &mut self.mission_hud {
                        mission.on_objective_state(*index, *state);
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
        let view = self.host().player_view(HUMAN);
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
        // M10.2 (PLAN §1.3): the cursor's order-feedback hint, resolved
        // before the renderer borrow (it reads the whole app: the cursor
        // position, the selection, the context-order machinery). The marker
        // quads build below, next to the other feedback cues.
        let cursor = self.cursor_ndc();
        let cursor_ground = self
            .camera
            .ground_point(glam::Vec2::new(cursor.0, cursor.1));
        let cursor_rally = self.selection_is_single_producer();
        let cursor_resolved = if cursor_rally {
            None
        } else {
            orders::resolve_context_order(
                &self.camera,
                &snapshot,
                glam::Vec2::new(cursor.0, cursor.1),
                HUMAN,
                self.node_kind,
                self.worker_kind,
                &self.selection,
            )
        };
        let cursor_hint = input::cursor_order_hint(
            cursor_resolved,
            cursor_rally,
            self.attack_move_armed,
            self.placement.is_some(),
            self.selection.is_empty(),
        );
        // M10.2 Phase 2 (PLAN §2.3): the hover name tooltip's data — resolved
        // before the renderer borrow, purely reading the snapshot. Passive:
        // it issues nothing and consumes no gesture state. Suppressed over
        // the bottom bar, where the cursor means UI, not world.
        let viewport = self
            .window
            .as_ref()
            .map(|window| {
                let size = window.inner_size();
                (size.width as f32, size.height as f32)
            })
            .unwrap_or((1920.0, 1080.0));
        let layout = ui::bar_layout(viewport);
        let cursor_px = feedback::ndc_to_pixels(cursor, viewport);
        let hover_info = if ui::cursor_over_bottom_bar(&layout, cursor_px.0, cursor_px.1) {
            None
        } else {
            orders::pick_visible_entity_sized(
                &self.camera,
                &snapshot.entities,
                glam::Vec2::new(cursor.0, cursor.1),
                |entity| {
                    // Big silhouettes read from farther out: structures get
                    // an extra reach proportional to their footprint.
                    ui::def_of(&self.bundle, entity.kind)
                        .and_then(|def| def.footprint())
                        .map(|(w, h)| (w.max(h) as f32) * 0.018)
                        .unwrap_or(0.0)
                },
            )
            .map(|entity| {
                let name = ui::def_of(&self.bundle, entity.kind)
                    .map(|def| def.display_name.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                (name, ui::owner_tag(entity.owner, HUMAN))
            })
        };
        let maps = self.map_list(); // before the renderer borrow
                                    // A-011: the mission list for the menu pass (the picker's field).
        let missions = self.mission_list();
        // A-002 Part 2.2: composed before the renderer borrow too (the
        // overlay's first line: build identity + live bundle's content hash).
        let version_line = self.version_line();
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
        // The HUD's `state_hash` is only read inside the F10 debug overlay,
        // so we use the cheap hud_state path every frame and only pay for
        // the canonical hash when the overlay is actually visible. The
        // hash is O(n) over the whole world — paying it every frame for a
        // value nothing else reads is a real waste on the renderer's hot
        // path.
        // (Field-style host access: `renderer` holds the mutable borrow.)
        let hud = match &self.host {
            Some(host) if self.debug_overlay => host.hud_state_with_hash(HUMAN),
            Some(host) => host.hud_state(HUMAN),
            None => HudState::default(),
        };
        let outcome = self.host.as_ref().and_then(|host| host.outcome());
        // M9: the health bars and command pings join the overlay pass
        // (plan §11.2's overlay layer). Projected through the same camera
        // the box select uses. (`viewport` and the bar layout were computed
        // above, before the renderer borrow — the tooltip shares them.)
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
            &self.selection,
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
        // M10.2 (PLAN §1.3): cursor feedback — a small ground marker naming
        // the order the next right-click (or armed left-click) would issue,
        // resolved above through the same context machinery that issues it.
        // It sticks to the ground under the cursor, not the pixel. The
        // command ping on submission stays (the marker previews, the ping
        // confirms).
        quads.extend(feedback::cursor_marker_quads(
            renderer.atlas(),
            cursor_hint,
            cursor_ground,
            &self.camera,
            viewport,
        ));
        // M10.2 Phase 2 (PLAN §2.3): the hover name tooltip — the display
        // name plus the owner tag, near the cursor, when the cursor rests
        // on a visible entity.
        if let Some((name, tag)) = &hover_info {
            quads.extend(ui::tooltip_quads(
                renderer.atlas(),
                name,
                tag,
                cursor_px,
                viewport,
            ));
        }
        // M10.2 (PLAN §1.3): the crosshair cursor while attack-move is
        // armed (the plan's "change the cursor" half; set only on change —
        // not every frame).
        let want_crosshair = self.attack_move_armed;
        if want_crosshair != self.crosshair_cursor {
            self.crosshair_cursor = want_crosshair;
            if let Some(window) = &self.window {
                window.set_cursor(if want_crosshair {
                    winit::window::CursorIcon::Crosshair
                } else {
                    winit::window::CursorIcon::Default
                });
            }
        }
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
            log_len: self.host.as_ref().map_or(0, |host| host.log().len()),
            viewport,
            armed: self.attack_move_armed,
            // A-011: the mission's objective HUD block (None = unchanged
            // mission-less HUD).
            mission: self.mission_hud.as_ref(),
            version: &version_line,
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
        // PLAN-M10.2 §3.5/§3.6: when a menu owns the screen (pause menu,
        // end screen, settings-over-pause), its pass dims the live world
        // and draws its panel on top; the built buttons are this frame's
        // hit rects.
        if !self.screen.takes_world_input() {
            // A-011 (ADR-0201 Phase 1, Task 4): a mission match's end screen
            // carries the mission's display name under the headline and each
            // objective's final state as info rows (objective left, state
            // right — the same block the Controls screen's table uses). A
            // mission-less match's end screen is unchanged.
            let mission_pairs: Vec<(String, String)> = self
                .mission_hud
                .as_ref()
                .map(|mission| mission.end_pairs())
                .unwrap_or_default();
            let mission_rows: Vec<(&str, &str)> = mission_pairs
                .iter()
                .map(|(text, state)| (text.as_str(), state.as_str()))
                .collect();
            let headline = outcome.as_ref().map(|outcome| {
                let head = match outcome.winner {
                    HUMAN => "VICTORY",
                    AI => "DEFEAT",
                    _ => "MUTUAL DESTRUCTION",
                };
                let sub = match &self.mission_hud {
                    Some(mission) => mission.display_name.as_str(),
                    None => match outcome.winner {
                        HUMAN => "You eliminated the enemy.",
                        AI => "Your base was destroyed.",
                        _ => "Both sides were eliminated.",
                    },
                };
                (head, sub)
            });
            // A-009 task 1: the Controls screen renders the standing binding
            // table (report.rs) as informational rows here too (when opened
            // from the pause menu — the in-match draw path serves the
            // pause menu and everything reachable from it).
            let info_lines: &[(&str, &str)] = match &self.screen.screen {
                Screen::Controls { .. } => report::CONTROLS_TABLE,
                Screen::EndScreen if !mission_rows.is_empty() => &mission_rows,
                _ => &[],
            };
            let built = ui::build_menu(ui::MenuInput {
                atlas: renderer.atlas(),
                viewport,
                screen: &self.screen,
                settings: &self.settings,
                maps: &maps,
                missions: &missions,
                end_lines: headline,
                backdrop: [0.0, 0.0, 0.0, 0.30],
                info_lines,
                // A-009 task 4: the pause menu (this in-match draw path)
                // also draws the build-identity footer. version_line was
                // composed above, before the renderer borrow.
                version: &version_line,
            });
            self.menu_buttons = built.buttons;
            quads.extend(built.quads);
        } else {
            self.menu_buttons.clear();
        }
        // A-011 (ADR-0201 §5): the cutscene's caption panel rides on top of
        // the frozen world frame (subtitle-style, bottom-centered, ASCII).
        if let Some(scene) = &self.cutscene {
            quads.extend(scene.caption_quads(renderer.atlas(), viewport));
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
pub(crate) fn panel_border(atlas: &TextAtlas, rect: [f32; 4], color: [f32; 4]) -> Vec<UiQuad> {
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
pub(crate) fn minimap_viewport_quads(
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
pub(crate) struct OverlayInput<'a> {
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
    /// A-002 Part 2.2: the version prefix line ("pandemonium X.Y git Z") —
    /// the F10 overlay's first line names the build (the A14 bug-report
    /// promise, automatic); the content hash joins it per frame from the
    /// live bundle.
    version: &'a str,
    /// A-011 (ADR-0201 Phase 1, Task 2): the live mission's tracker — the
    /// objective HUD block draws under the notice lines (None = no mission,
    /// the HUD unchanged).
    mission: Option<&'a ui::MissionHud>,
}

/// Builds the overlay quads for one frame: the resource/population HUD
/// (plan §11.4's minimal slice), the §11.6 debug overlay (when toggled), and
/// (M8) the end-screen panel when the match has resolved.
pub(crate) fn overlay_quads(input: OverlayInput<'_>) -> Vec<UiQuad> {
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
        version,
        mission,
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

    // A-011 (ADR-0201 Phase 1, Task 2): the mission's objective HUD — one
    // panel under the notice lines, one line per objective with its state
    // marker (pending / completed / failed). A mission-less match draws
    // nothing here (the block is gated on the tracker's presence).
    if let Some(mission) = mission {
        let (block, height) = ui::objective_hud_quads(atlas, mission, notice_top);
        quads.extend(block);
        notice_top += height + MARGIN;
    }

    // The debug overlay (§11.6), below the notice lines.
    if debug {
        let lines = [
            // A-002 Part 2.2: the build identity + the LIVE bundle's content
            // hash (the match's own — a started match on another map shows
            // that map's bundle hash; the overlay never lies about which
            // content is running). The caller composes the whole line.
            version.to_string(),
            format!("tick {}", hud.tick),
            format!("hash {:#018x}", hud.state_hash),
            format!("entities {entities}  selected {selected}  log {log_len}"),
            if hud.paused {
                "PAUSED  [.] step  [F11] bug report".to_string()
            } else {
                "[F10] overlay  [P] pause  [F11] bug report".to_string()
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

/// The git short SHA stamped by build.rs at compile time ("unknown" for a
/// build from a tarball — an honest string, never a failure).
pub(crate) fn git_sha() -> &'static str {
    env!("PANDEMONIUM_GIT_SHA")
}

/// The kind ids (content's `id` strings), aligned with [`kind_shapes`] by
/// index — the renderer's pack lookup keys by these strings while the
/// silhouette specs key by KindId index. Pure presentation plumbing: the
/// names come from the loaded bundle and never feed back into content.
pub(crate) fn kind_names(bundle: &ContentBundle) -> Vec<String> {
    bundle.entities.iter().map(|def| def.id.clone()).collect()
}

/// PLAN-M11 §2 (phase 2): the content's combat kinds — every kind whose
/// definition carries an Attack capability. Q's filter, computed once per
/// match over the bundle (the same capability shape the engine's plan
/// resolution and `kind_shapes` follow — never name-matched).
pub(crate) fn combat_kinds(bundle: &ContentBundle) -> BTreeSet<KindId> {
    bundle
        .entities
        .iter()
        .enumerate()
        .filter(|(_, def)| def.capability("Attack").is_some())
        .map(|(index, _)| KindId(index as u32))
        .collect()
}

/// PLAN-M11 §2 (phase 2): the content's unit kinds — every kind whose
/// definition carries a Move capability (workers, soldiers; structures
/// and nodes have footprints instead). The Ctrl+Left/Right cycle's
/// candidate filter, computed once per match.
pub(crate) fn unit_kinds(bundle: &ContentBundle) -> BTreeSet<KindId> {
    bundle
        .entities
        .iter()
        .enumerate()
        .filter(|(_, def)| def.capability("Move").is_some())
        .map(|(index, _)| KindId(index as u32))
        .collect()
}

/// PLAN-M11 §2 (phase 2): the command-center kind through the engine
/// plan's own rule (`alpha_plan`'s producer scan — the first producer
/// whose producibles include no attacking kind), kept here so H reads a
/// `None` when the faction has no such producer instead of the plan's
/// `KindId(0)` fallback (kind 0 is the barracks on this content — a
/// wrong jump target). Capability-shaped, never name-matched.
pub(crate) fn command_center_kind(bundle: &ContentBundle) -> Option<KindId> {
    let entities = &bundle.entities;
    let kind_of = |id: &str| {
        entities
            .iter()
            .position(|entity| entity.id == id)
            .map(|index| KindId(index as u32))
    };
    let attacks = |kind: KindId| {
        entities
            .get(kind.0 as usize)
            .is_some_and(|def| def.capability("Attack").is_some())
    };
    for (producer, producibles) in &bundle.faction().production {
        let Some(producer_kind) = kind_of(producer) else {
            continue;
        };
        let kinds: Vec<KindId> = producibles.iter().filter_map(|p| kind_of(p)).collect();
        if !kinds.iter().any(|kind| attacks(*kind)) {
            return Some(producer_kind);
        }
    }
    None
}

/// Derives one multi-part silhouette per kind from the loaded bundle
/// (PLAN-M10.2 §2.1): the kind-name table resolves the nine authored kinds
/// to their hand-tuned box compositions; anything the table does not know
/// falls back by capability shape (Resource → amber cluster, Footprint →
/// building box, Move → unit box) — the same capability-shaped law the
/// engine's plan resolution follows. No presentation data touches the
/// content files.
pub(crate) fn kind_shapes(bundle: &ContentBundle) -> Vec<render::KindShape> {
    bundle
        .entities
        .iter()
        .map(|def| {
            silhouette::named(&def.id).unwrap_or_else(|| {
                silhouette::capability_fallback(
                    def.footprint(),
                    def.capability("Move").is_some(),
                    def.capability("Attack").is_some(),
                    def.capability("Resource").is_some(),
                )
            })
        })
        .collect()
}
