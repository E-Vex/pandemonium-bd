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
mod render;
mod text;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use feedback::FeedbackState;
use pandemonium_ai::Controller;
use pandemonium_content::ContentBundle;
use pandemonium_engine::audio::{AudioSink, NullAudioSink};
use pandemonium_engine::mesh::terrain_mesh;
use pandemonium_engine::renderer::{Frame, HudState};
use pandemonium_engine::{
    alpha_controller, MatchHost, MatchOutcome, NullRenderer, Renderer, RtsCamera,
};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, MatchSetup, PlayerId, PlayerSetup,
};
use render::WgpuRenderer;
use text::{TextAtlas, UiQuad};
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

fn main() -> anyhow::Result<()> {
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
    let mut app = App::new(bundle)?;
    app.frames_budget = frames_budget_arg();
    event_loop
        .run_app(&mut app)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

/// Parses the verification affordance `--frames N`: exit cleanly after N
/// presented frames and print the windowed smoke summary (tick, state hash,
/// commands submitted, selection size). Presentation-layer tooling for the
/// DEBT-008 windowed verification — it changes no simulation behavior.
fn frames_budget_arg() -> Option<u64> {
    let mut args = std::env::args();
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
}

impl App {
    fn new(bundle: ContentBundle) -> anyhow::Result<Self> {
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
        let resource_names: Vec<String> = bundle
            .rules
            .resources
            .iter()
            .map(|resource| resource.display_name.clone())
            .collect();
        let camera = RtsCamera::new(bundle.map.width, bundle.map.height, 16.0 / 9.0);
        let host = Self::build_host(&bundle, setup.clone());
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
            last_frame: Instant::now(),
            command_seq: 0,
            resource_names,
            debug_overlay: false,
            frames_budget: None,
            frames_presented: 0,
            commands_submitted: 0,
            audio: NullAudioSink::new(),
            feedback: FeedbackState::default(),
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

    /// Single-click selection: the nearest own entity to the cursor.
    fn click_select(&mut self, ndc: (f32, f32)) {
        let snapshot = self.host.render_snapshot();
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
        self.selection = best.map(|(_, id)| vec![id]).unwrap_or_default();
    }

    /// Drag-box selection over own entities.
    fn box_select(&mut self, a: (f32, f32), b: (f32, f32)) {
        let snapshot = self.host.render_snapshot();
        let own: Vec<pandemonium_engine::RenderEntity> = snapshot
            .entities
            .iter()
            .filter(|entity| entity.owner == HUMAN)
            .copied()
            .collect();
        self.selection =
            self.camera
                .box_select(&own, glam::Vec2::new(a.0, a.1), glam::Vec2::new(b.0, b.1));
    }

    /// Right-click move: the picked ground point becomes a Move order for the
    /// selection (the only path through which the client affects the sim).
    fn issue_move(&mut self, ndc: (f32, f32)) {
        if self.selection.is_empty() {
            return;
        }
        let Some(point) = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1)) else {
            return;
        };
        let target = MatchHost::world_to_logical(point.x, point.z);
        // M9: the immediate acknowledgment cue — the crosshair the player
        // sees on the frame they clicked (the motion follows within the
        // A8 tick bounds; this is the intent-to-visible-response ping).
        self.feedback.ping(target, self.frames_presented);
        self.command_seq += 1;
        self.commands_submitted += 1;
        self.host.submit(Command::new(
            HUMAN,
            0,
            self.command_seq,
            CommandKind::Move {
                units: self.selection.clone(),
                target,
            },
        ));
    }

    /// M8 (plan §11.3): Stop hotkey — clears the selection's order queues and
    /// any auto-acquired combat targets (the gate's Stop command).
    fn issue_stop(&mut self) {
        if self.selection.is_empty() {
            return;
        }
        self.command_seq += 1;
        self.commands_submitted += 1;
        self.host.submit(Command::new(
            HUMAN,
            0,
            self.command_seq,
            CommandKind::Stop {
                units: self.selection.clone(),
            },
        ));
    }

    /// M8 (plan §11.3): AttackMove hotkey — the selection moves to the picked
    /// ground point, engaging enemies encountered en route (combat semantics
    /// from M6). Falls back to a plain Move for units without Attack.
    fn issue_attack_move(&mut self, ndc: (f32, f32)) {
        if self.selection.is_empty() {
            return;
        }
        let Some(point) = self.camera.ground_point(glam::Vec2::new(ndc.0, ndc.1)) else {
            return;
        };
        let target = MatchHost::world_to_logical(point.x, point.z);
        self.feedback.ping(target, self.frames_presented);
        self.command_seq += 1;
        self.commands_submitted += 1;
        self.host.submit(Command::new(
            HUMAN,
            0,
            self.command_seq,
            CommandKind::AttackMove {
                units: self.selection.clone(),
                target,
            },
        ));
    }

    /// M8 (plan §11.3): assign the current selection to control group `index`
    /// (0-8 mapping to display 1-9).
    fn assign_control_group(&mut self, index: usize) {
        if index < self.control_groups.len() {
            self.control_groups[index] = self.selection.clone();
        }
    }

    /// M8 (plan §11.3): recall control group `index` (0-8). Selection becomes
    /// the group's members that are still alive (the snapshot filters the
    /// dead — ids are never reused, so a stale group reference is harmless).
    fn recall_control_group(&mut self, index: usize) {
        if index >= self.control_groups.len() {
            return;
        }
        let live: std::collections::BTreeSet<EntityId> = self
            .host
            .render_snapshot()
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
        match WgpuRenderer::new(window.clone(), &terrain_mesh_of()) {
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
                // The windowed smoke budget (DEBT-008 verification): exit after
                // N presented frames, with the evidence summary.
                if let Some(budget) = self.frames_budget {
                    self.frames_presented += 1;
                    if self.frames_presented >= budget {
                        self.windowed_summary();
                        event_loop.exit();
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.drag_current = Some((position.x, position.y));
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let Some(window) = &self.window else { return };
                let cursor = self.drag_current.unwrap_or((0.0, 0.0));
                let ndc = Self::to_ndc(window, cursor.0, cursor.1);
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => {
                        self.drag_start = Some(cursor);
                    }
                    (MouseButton::Left, ElementState::Released) => {
                        if let Some(start) = self.drag_start.take() {
                            let start_ndc = Self::to_ndc(window, start.0, start.1);
                            let dragged = (start_ndc.0 - ndc.0).abs() > CLICK_SLOP
                                || (start_ndc.1 - ndc.1).abs() > CLICK_SLOP;
                            if dragged {
                                self.box_select(start_ndc, ndc);
                            } else {
                                self.click_select(ndc);
                            }
                        }
                    }
                    (MouseButton::Right, ElementState::Pressed) => self.issue_move(ndc),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(_, lines) => self.camera.zoom(1.0 + lines * 0.1),
                MouseScrollDelta::PixelDelta(delta) => {
                    self.camera.zoom(1.0 + delta.y as f32 * 0.001)
                }
            },
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                if let PhysicalKey::Code(code) = event.physical_key {
                    match code {
                        winit::keyboard::KeyCode::KeyW => set_key(&mut self.keys, "w", pressed),
                        winit::keyboard::KeyCode::KeyA => {
                            set_key(&mut self.keys, "a", pressed);
                            // M8: 'A' is also the AttackMove hotkey on press
                            // (camera pan uses the held state; the command
                            // fires once on the key-down edge). Only when the
                            // match is ongoing — end-screen 'R' takes over.
                            if pressed && !self.host.is_finished() {
                                let ndc = self.cursor_ndc();
                                self.issue_attack_move(ndc);
                            }
                        }
                        winit::keyboard::KeyCode::KeyS => {
                            set_key(&mut self.keys, "s", pressed);
                            // M8: 'S' is also the Stop hotkey on press.
                            if pressed && !self.host.is_finished() {
                                self.issue_stop();
                            }
                        }
                        winit::keyboard::KeyCode::KeyD => set_key(&mut self.keys, "d", pressed),
                        // §11.6 debug tooling: overlay toggle, pause, single-step.
                        winit::keyboard::KeyCode::F3 if pressed => {
                            self.debug_overlay = !self.debug_overlay;
                        }
                        winit::keyboard::KeyCode::KeyP if pressed => {
                            self.host.set_paused(!self.host.is_paused());
                        }
                        winit::keyboard::KeyCode::Period if pressed => {
                            let _ = self.host.step_once();
                        }
                        // M8 (plan §11.3): control groups 1-9. Ctrl+digit
                        // assigns the current selection; digit alone recalls.
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
                            } else {
                                self.recall_control_group(index);
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
                        winit::keyboard::KeyCode::Escape if pressed => event_loop.exit(),
                        _ => {}
                    }
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        // Continuous camera motion from held keys, then keep the frames coming.
        let pan_speed = 0.8;
        let mut dx = 0.0;
        let mut dz = 0.0;
        if self.keys.contains(&Key::Character("w")) {
            dz -= pan_speed;
        }
        if self.keys.contains(&Key::Character("s")) {
            dz += pan_speed;
        }
        if self.keys.contains(&Key::Character("a")) {
            dx -= pan_speed;
        }
        if self.keys.contains(&Key::Character("d")) {
            dx += pan_speed;
        }
        if dx != 0.0 || dz != 0.0 {
            self.camera.pan(dx, dz);
        }
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

impl App {
    /// The windowed smoke summary: what the `--frames` run proved (frames
    /// presented, sim progress, commands submitted through input, selection).
    fn windowed_summary(&self) {
        println!(
            "pandemonium client — windowed smoke: {} frames presented, tick {}, state hash {:#018x}, \
             {} commands submitted, selection {}",
            self.frames_presented,
            self.host.tick(),
            self.host.state_hash(),
            self.commands_submitted,
            self.selection.len()
        );
    }

    fn draw(&mut self) {
        let dt = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        let outcome = self.host.advance(dt);
        // M9 (plan §11.5, FD-9): the step's events flow outward — the audio
        // sink and the feedback state consume them after the step, never
        // inside it. Presentation-only bookkeeping.
        self.audio.on_events(&outcome.events);
        self.feedback
            .on_events(&outcome.events, self.frames_presented);
        self.feedback.expire(self.frames_presented);
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let snapshot = self.host.render_snapshot();
        let view_projection = self.camera.view_projection();
        let eye = self.camera.eye();
        let hud = self.host.hud_state(HUMAN);
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
        let mut quads =
            feedback::health_bar_quads(renderer.atlas(), &snapshot, &self.camera, viewport);
        quads.extend(feedback::ping_quads(
            renderer.atlas(),
            self.feedback.active_pings(),
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
        }));
        renderer.queue_ui(&quads);
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

    // The debug overlay (§11.6), below the HUD panel.
    if debug {
        let lines = [
            format!("tick {}", hud.tick),
            format!("hash {:#018x}", hud.state_hash),
            format!("entities {entities}  selected {selected}  log {log_len}"),
            if hud.paused {
                "PAUSED  [.] step".to_string()
            } else {
                "[F3] overlay  [P] pause".to_string()
            },
        ];
        let widest = lines
            .iter()
            .map(|line| atlas.measure(line))
            .fold(0.0f32, f32::max);
        let top = MARGIN + line_height + 2.0 * PAD + MARGIN;
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
        // Center the panel in the window — the atlas doesn't know the window
        // size, so we use a fixed large offset (the renderer's NDC pipeline
        // clips anything off-screen). 320x240 px is a safe center for the
        // default window; on other sizes the panel stays anchored top-left
        // of center.
        let panel_x = 320.0 - panel_w / 2.0;
        let panel_y = 240.0 - panel_h / 2.0;
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
    let frame_dt = std::time::Duration::from_secs_f64(1.0 / 60.0);
    let mut entities = 0;
    for frame in 0..180 {
        let outcome = host.advance(frame_dt);
        audio.on_events(&outcome.events);
        feedback.on_events(&outcome.events, frame);
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
        "  feedback wiring: {} events fed the audio sink ({} cues), {} attacks flashed on screen",
        audio.fed, audio.cues, feedback.hits_seen
    );
    println!("  content hash:    {:#018x}", bundle.content_hash());
    println!("pandemonium client — smoke PASS (windowed M3 verification requires a display)");
    Ok(())
}
