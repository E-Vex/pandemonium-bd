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

mod app;
mod config;
mod cutscene;
mod feedback;
#[cfg(test)]
mod hw;
mod input;
use input::event::{digit_to_group_index, set_key};

mod render;
mod report;
mod screens;
mod silhouette;
mod sound;
mod text;
mod ui;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use app::App;
use feedback::FeedbackState;
use input::RightRelease;
use pandemonium_content::{ContentBundle, ContentTree};
use pandemonium_engine::audio::AudioSink;
use pandemonium_engine::mesh::terrain_mesh;
use pandemonium_engine::renderer::Frame;
use pandemonium_engine::{MatchHost, NullRenderer, Renderer};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, Event, MatchSetup, PlayerId, PlayerSetup,
    Vec2Fx,
};
use render::WgpuRenderer;
use screens::{Effect, MatchMode};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, PhysicalKey};
use winit::window::{Window, WindowId};

// Test-only reaches: `report.rs`'s test module resolves
// `crate::alpha_controller` through the crate root, and this file's test
// modules pull their screens vocabulary through `use super::*`.
#[cfg(test)]
use pandemonium_engine::alpha_controller;
#[cfg(test)]
use screens::Screen;

/// The player the human controls.
pub(crate) const HUMAN: PlayerId = PlayerId(0);
/// The AI opponent's slot.
pub(crate) const AI: PlayerId = PlayerId(1);
/// How far the cursor can drift (in NDC) for a press-release to count as a
/// click rather than a drag.
pub(crate) const CLICK_SLOP: f32 = 0.01;
/// Heightmap units -> tiles of vertical displacement (display-only, ADR-0001).
pub(crate) const HEIGHT_SCALE: f32 = 0.02;
/// How many control groups the client tracks (plan §11.3: control groups).
pub(crate) const CONTROL_GROUP_COUNT: usize = 9;
/// Camera pan speed comes from the settings file now (PLAN-M10.2 §3.4:
/// `pan_speed`, default [`config::DEFAULT_PAN_SPEED`]) — keys and the edge
/// band share it, scaled by the real frame delta so pan speed does not
/// depend on the display's refresh rate.
/// Camera yaw rotation speed in radians per second (Generals-style Q/E
/// camera rotation). Held keys apply this rate, scaled by the frame
/// delta, so a full turn takes ~6 s — fast enough to re-orient without
/// inducing motion sickness.
pub(crate) const ROTATE_RAD_PER_SEC: f32 = pandemonium_engine::camera::DEFAULT_ROTATE_RAD_PER_SEC;
/// Pitch adjustment per wheel notch when Ctrl is held (Generals-style
/// Ctrl+wheel pitch control). ~12 notches cover the full pitch range.
pub(crate) const PITCH_PER_WHEEL_NOTCH: f32 =
    pandemonium_engine::camera::DEFAULT_PITCH_PER_WHEEL_NOTCH;
/// The frame-delta clamp for camera motion — a stall must not teleport the
/// view when the loop resumes.
pub(crate) const MAX_PAN_FRAME_SECONDS: f32 = 0.05;
/// The opening camera distance from the player's start anchor (M9.1: the
/// old map-center default at 58 tiles out rendered the starting force as
/// specks — "nothing appears on the screen").
pub(crate) const START_CAMERA_DISTANCE: f32 = 26.0;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let seed_flag = seed_flag_from_args(&args);
    let record_path = record_from_args(&args);
    // A-002 Part 2.1: the content tree resolves at RUNTIME now — the flag,
    // then the executable's own directory (the release archive layout:
    // binary and content/ side by side, content external so the bundle hash
    // is untouched), then the compile-time workspace path (cargo run's dev
    // layout). `CARGO_MANIFEST_DIR` alone was dev-only: a binary run from
    // any other directory (or any other machine) could never find it.
    let content_result = content_dir_from(&args);
    // A-002 Part 2.2: `--version` answers even on a broken install — the
    // version line is the support affordance, the content hash joins it
    // when the content tree loads and says so honestly when it cannot.
    if version_requested_from_args(&args) {
        let content_hash = match &content_result {
            Ok(content) => ContentBundle::load_dir(content)
                .ok()
                .map(|bundle| format!("{:#018x}", bundle.content_hash())),
            Err(candidates) => {
                eprintln!("pandemonium client — {}", content_search_error(candidates));
                None
            }
        }
        .unwrap_or_else(|| "unavailable (content did not load)".to_string());
        println!(
            "pandemonium-client {} (git {}, content {})",
            env!("CARGO_PKG_VERSION"),
            app::git_sha(),
            content_hash
        );
        return Ok(());
    }
    let content = content_result.unwrap_or_else(|candidates| {
        eprintln!("pandemonium client — {}", content_search_error(&candidates));
        std::process::exit(1);
    });
    // PLAN-M10.2 §3.3: the whole tree loads — the New Match screen lists its
    // maps, and a match bundles the selected one. The Alpha ships one map;
    // the default bundle (its first map) backs the menu's backdrop and the
    // headless smoke pass.
    let tree = ContentTree::load_dir(&content)
        .with_context(|| format!("loading content from {}", content.display()))?;
    let default_bundle = tree
        .bundle(
            &tree
                .maps
                .first()
                .map(|map| map.id.clone())
                .ok_or_else(|| anyhow::anyhow!("the content tree lists no maps"))?,
        )
        .with_context(|| "bundling the default map")?;

    // A-002 Part 2.3: `--headless` forces the smoke pass without consulting
    // the display — CI runners and display-less machines say what they want
    // instead of discovering the M3 fallback by accident (a Windows runner
    // HAS a desktop session, so the natural fallback would never trigger
    // there — and a windowed run without --frames would never exit).
    if headless_requested_from_args(&args) {
        println!("pandemonium client — headless requested (--headless); running the smoke pass");
        return headless_smoke(&default_bundle);
    }

    let event_loop = match EventLoop::builder().build() {
        Ok(event_loop) => event_loop,
        Err(error) => {
            // The M3 finding, printed honestly: the windowed experience needs a
            // display; CI and other headless machines get the smoke pass.
            println!(
                "pandemonium client — no display available ({error}); running the headless \
                 smoke pass (windowed mode needs X11/Wayland; see AI-Handoff §8)"
            );
            return headless_smoke(&default_bundle);
        }
    };
    // PLAN-M10.2 §3.2: the game starts at the main menu — the match host is
    // deferred until Start is pressed (the A15 drop + reconstruct path,
    // unchanged determinism).
    // PLAN-M10.2 §3.4's evidence line: where the settings came from (the
    // persistence-across-runs proof the re-test and the smoke runs read).
    match config::config_path() {
        Some(path) => println!(
            "pandemonium client — settings: {} ({} the file)",
            path.display(),
            if path.is_file() {
                "loaded"
            } else {
                "absent, defaults"
            }
        ),
        None => println!(
            "pandemonium client — settings: no config directory, in-memory defaults (persistence \
             disabled)"
        ),
    }
    // A-002 (BRIEF Part 1): the settings load ONCE here and both the audio
    // probe and the App consume it. The App no longer probes the device in
    // its constructor — the wiring layer owns WHEN (if ever) the probe runs.
    // Unit tests construct the App with `Sound::without_device`, so a test
    // binary never opens an audio output stream (five concurrent
    // `audio_wiring` apps each used to probe the real device under cargo
    // test: a leak of the machine into the test environment, and — on a
    // Windows runner with an unusual audio stack — the prime suspect for the
    // STATUS_ACCESS_VIOLATION the CI legs have shown since run #38).
    let settings = config::load();
    let audio = sound::Sound::new(settings.master_volume, settings.muted);
    let mut app = App::new(
        tree,
        default_bundle,
        seed_flag,
        record_path,
        settings,
        audio,
    )?;
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
/// the windowed player to be able to name the seed they played).
/// PLAN-M10.2 §3.3: the value pre-fills the New Match screen's seed field;
/// when the flag is absent (or malformed) the field starts *random* — the
/// plan's own default — so `Some(N)` means "the player asked for this seed"
/// and `None` means "roll one" (the OS-entropy source, A-111).
fn seed_flag_from_args(args: &[String]) -> Option<u64> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--seed" {
            return args.next().and_then(|value| value.parse::<u64>().ok());
        }
    }
    None
}

/// A fresh seed from the OS entropy source available in std (A-111):
/// `RandomState` is seeded from the OS per construction, so two fresh
/// hashers finish to different values. UI-only — this fills the New Match
/// screen's seed field and never touches the sim's deterministic RNG.
fn random_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
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

/// A-002 Part 2.1: the runtime content search. Order: the `--content <dir>`
/// flag (the player's explicit word — the only candidate when present), then
/// `<exe_dir>/content` (the release archive layout: binary and content side
/// by side, content external, the bundle hash unaffected), then the
/// compile-time workspace path (the cargo-run dev layout; last, because a
/// binary built elsewhere points at another machine's filesystem).
///
/// Pure function over its inputs so the law is unit-testable; the wrapper
/// below is the only part that touches the real argv and executable.
fn resolve_content_dir(explicit: Option<&Path>, exe: &Path) -> Result<PathBuf, Vec<PathBuf>> {
    let candidates: Vec<PathBuf> = match explicit {
        Some(dir) => vec![dir.to_path_buf()],
        None => {
            let exe_dir = exe.parent().unwrap_or_else(|| Path::new("."));
            vec![exe_dir.join("content"), PathBuf::from(content_path())]
        }
    };
    for candidate in &candidates {
        if candidate.is_dir() {
            return Ok(candidate.clone());
        }
    }
    Err(candidates)
}

/// The one-line human form of a failed search (A-002 Part 2.1: "print a
/// precise error listing paths searched").
fn content_search_error(candidates: &[PathBuf]) -> String {
    let mut message = String::from("content directory not found. Searched:");
    for candidate in candidates {
        message.push_str(&format!("\n  {}", candidate.display()));
    }
    message.push_str("\nPass --content <dir> to name the tree explicitly.");
    message
}

/// Resolves the content directory for THIS process (flag, then executable,
/// then dev path). A failed search is an `Err` of every path tried — main
/// turns it into the precise error and exit 1, and `--version` degrades
/// around it (the version line must answer on a broken install too).
fn content_dir_from(args: &[String]) -> Result<PathBuf, Vec<PathBuf>> {
    let explicit = content_flag_from_args(args);
    let exe = std::env::current_exe().unwrap_or_else(|error| {
        // An exotic failure (deleted binary, /proc-less container): fall
        // back to a pathless probe — the search then tries the compiled-in
        // dev path, and the error (if any) will still be precise.
        eprintln!(
            "pandemonium client — could not locate the own executable ({error}); trying the \
             compiled-in dev path"
        );
        PathBuf::from(".")
    });
    resolve_content_dir(explicit.as_deref(), &exe)
}

/// Parses the content override `--content <dir>` (A-002 Part 2.1). A missing
/// or malformed value is a parse failure — an override the player typed must
/// be honored exactly or refused loudly, never silently ignored.
fn content_flag_from_args(args: &[String]) -> Option<PathBuf> {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--content" {
            return args.next().map(PathBuf::from);
        }
    }
    None
}

/// Parses the version affordances `--version` / `-V` (A-002 Part 2.2).
fn version_requested_from_args(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--version" || arg == "-V")
}

/// Parses the headless affordance `--headless` (A-002 Part 2.3): force the
/// smoke pass — no display probing, no window, no event loop.
fn headless_requested_from_args(args: &[String]) -> bool {
    args.iter().any(|arg| arg == "--headless")
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        // A-002 Part 1(b): a player whose windowing environment cannot create
        // the window gets a logged, precise error and a clean exit — the
        // same contract the GPU path below already honours (never a panic,
        // never an abort).
        let window = match event_loop
            .create_window(Window::default_attributes().with_title("Pandemonium"))
        {
            Ok(window) => Arc::new(window),
            Err(error) => {
                eprintln!("pandemonium client — window creation failed: {error:#}");
                event_loop.exit();
                return;
            }
        };
        self.camera.set_aspect(
            window.inner_size().width as f32 / window.inner_size().height.max(1) as f32,
        );
        match WgpuRenderer::new(
            window.clone(),
            &terrain_mesh(&self.tree.maps[0], HEIGHT_SCALE),
            crate::app::kind_shapes(&self.bundle),
        ) {
            Ok(renderer) => {
                self.renderer = Some(renderer);
                self.window = Some(window);
                // PLAN-M10.2 §3.4: the persisted fullscreen preference
                // applies at startup too (not only at toggle time).
                self.apply_fullscreen();
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
                // A-009 task 3: minimize is signaled on the pinned winit
                // (0.30.13) by `WindowEvent::Resized(PhysicalSize::ZERO)`.
                // winit 0.30 has no dedicated Minimized variant; the
                // (0,0) resize is the standard 0.30 signal (the platform
                // backends emit it on minimize — see winit's own
                // event.rs:1043-1045 in this version). The auto-pause
                // decision is a pure function over (screen, mode, paused,
                // event) — see `screens::auto_pause_on_focus` — and the
                // wiring here applies Effect::Pause through the existing
                // `AppState::open_pause()` path (which both pauses the
                // host and opens the pause menu). The --frames/smoke runs
                // never emit this signal under Xvfb (no window manager
                // minimizes the window), so the behavior is unchanged.
                if size.width == 0 && size.height == 0 {
                    if let Some(host) = &self.host {
                        let effect = screens::auto_pause_on_focus(
                            &self.screen.screen,
                            self.match_mode,
                            host.is_paused(),
                            screens::FocusEvent::Minimized,
                        );
                        if matches!(effect, Effect::Pause) {
                            let (next, effect) = self.screen.open_pause();
                            self.screen = next;
                            self.apply_effect(effect, event_loop);
                        }
                    }
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
                // PLAN-M10.2 §3.6: menus own the pointer — hover moves the
                // keyboard focus so mouse and keyboard share one navigation
                // core. (The button rects arrive with the menu pass.)
                if !self.screen.takes_world_input() {
                    self.menu_hover();
                    return;
                }
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
                // A-011 (ADR-0201 §5): an active cutscene owns the buttons
                // too — clicks neither skip the scene (any *key* does, per
                // the brief) nor reach the world (nothing may order while
                // the host is paused under the scene).
                if self.cutscene.is_some() {
                    return;
                }
                // PLAN-M10.2 §3.6: menus own the buttons — a left press
                // activates the hovered row; nothing in a menu may send a
                // sim command. The other buttons are inert at a menu.
                if !self.screen.takes_world_input() {
                    if (button, state) == (MouseButton::Left, ElementState::Pressed) {
                        self.menu_click(event_loop);
                    }
                    return;
                }
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
                                // replacing it). The one selection gate
                                // (A-122) cues the change.
                                let picked = self.box_pick(start_ndc, ndc);
                                let next = if shift {
                                    input::union_selection(&self.selection, &picked)
                                } else {
                                    picked
                                };
                                self.set_selection(next);
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
                                        let next = if shift {
                                            input::union_selection(&self.selection, &same_kind)
                                        } else {
                                            same_kind
                                        };
                                        self.set_selection(next);
                                    } else if shift {
                                        // Shift+click toggles membership.
                                        let next =
                                            input::shift_click_selection(&self.selection, picked);
                                        self.set_selection(next);
                                    } else {
                                        self.set_selection(vec![picked]);
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
                // A-011: no zooming while a cutscene drives the camera.
                if self.cutscene.is_some() {
                    return;
                }
                if !self.screen.takes_world_input() {
                    return; // menus own the wheel
                }
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
                    // PLAN-M10.2 §3.4: the user zoom limits apply after the
                    // engine's own clamp (they can only narrow it).
                    self.camera
                        .clamp_distance_to(self.settings.zoom_min, self.settings.zoom_max);
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
                // A-011 (ADR-0201 §5 (iii)): an active cutscene owns the
                // keyboard — any key, Esc included, ends the scene and
                // unpauses (the mission-start pan is six seconds; nothing
                // else fires while it runs). Key releases just clear the
                // held-key set so a resumed match never pans on a ghost.
                if self.cutscene.is_some() {
                    if pressed {
                        self.cutscene_input();
                    } else {
                        self.keys.clear();
                    }
                    return;
                }
                if let PhysicalKey::Code(code) = event.physical_key {
                    // PLAN-M10.2 §3.6: menus own the keyboard — up/down moves
                    // focus, Enter activates, Esc backs out (the pure
                    // machine). Stale camera keys clear so a resumed match
                    // never pans on a ghost of a held key.
                    if !self.screen.takes_world_input() {
                        if pressed {
                            self.menu_key(code, event_loop);
                        } else {
                            self.keys.clear();
                        }
                        return;
                    }
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
                            if pressed && !self.host.as_ref().is_some_and(|h| h.is_finished()) {
                                self.attack_move_armed = true;
                            }
                        }
                        winit::keyboard::KeyCode::ArrowLeft => {
                            set_key(&mut self.keys, "a", pressed)
                        }
                        winit::keyboard::KeyCode::KeyS => {
                            set_key(&mut self.keys, "s", pressed);
                            // M8: 'S' is also the Stop hotkey on press.
                            if pressed && !self.host.as_ref().is_some_and(|h| h.is_finished()) {
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
                            if let Some(host) = &mut self.host {
                                host.set_paused(!host.is_paused());
                            }
                        }
                        winit::keyboard::KeyCode::Period if pressed => {
                            if let Some(host) = &mut self.host {
                                let outcome = host.step_once();
                                self.collect_checkpoints(&outcome);
                            }
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
                            // The end-screen rematch shortcut for the sliver
                            // between the match ending and the draw-time
                            // promotion; the end screen itself owns R after.
                            if self.host.as_ref().is_some_and(|h| h.is_finished()) {
                                self.restart_match();
                            }
                        }
                        winit::keyboard::KeyCode::Escape if pressed => {
                            // M10.2 (PLAN §1.3): Escape climbs the ladder
                            // one rung per press — armed command, then
                            // placement, then the selection, and only then
                            // quit (the old code quit on the first press
                            // whenever nothing was armed; an accidental
                            // Esc with units selected cost the player
                            // their window, not just their selection).
                            match input::escape_action(
                                self.attack_move_armed,
                                self.placement.is_some(),
                                !self.selection.is_empty(),
                            ) {
                                input::EscapeAction::CancelArmed => {
                                    self.attack_move_armed = false;
                                }
                                input::EscapeAction::CancelPlacement => {
                                    self.placement = None;
                                }
                                input::EscapeAction::ClearSelection => {
                                    // Through the selection gate (A-122):
                                    // a clear of a non-empty selection is a
                                    // change, and it clicks.
                                    self.set_selection(Vec::new());
                                }
                                // PLAN-M10.2 §3.5: the ladder's exhausted
                                // rung opens the pause menu (it quit the
                                // app before). The match pauses through
                                // the existing MatchHost pause.
                                input::EscapeAction::OpenPauseMenu => {
                                    let (next, effect) = self.screen.open_pause();
                                    self.screen = next;
                                    self.apply_effect(effect, event_loop);
                                }
                            }
                        }
                        // M10.2 (PLAN §1.3, the "only if cheap" extra):
                        // Space jumps the camera to the last event's
                        // position (the most recent death — the class of
                        // event a player most wants to look at), else the
                        // player's base. The zoom distance is kept.
                        winit::keyboard::KeyCode::Space if pressed => {
                            let (x, z) =
                                input::jump_target(self.last_event_ground, self.start_anchor);
                            let distance = self.camera.distance();
                            self.camera.focus(x, z, distance);
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
                // A-009 task 3: auto-pause on focus loss during a live
                // human-driven match (the classic-RTS contract). The pure
                // decision is in `screens::auto_pause_on_focus`; the wiring
                // here applies Effect::Pause through the existing
                // `AppState::open_pause()` path (host pause + pause menu).
                // Regaining focus (Focused::true) does NOT auto-resume —
                // the player resumes explicitly through the pause menu.
                // The --frames/smoke runs under Xvfb have no window
                // manager to steal focus, so the behavior is unchanged.
                let event = if focused {
                    screens::FocusEvent::Focused
                } else {
                    screens::FocusEvent::Unfocused
                };
                if let Some(host) = &self.host {
                    let effect = screens::auto_pause_on_focus(
                        &self.screen.screen,
                        self.match_mode,
                        host.is_paused(),
                        event,
                    );
                    if matches!(effect, Effect::Pause) {
                        let (next, effect) = self.screen.open_pause();
                        self.screen = next;
                        self.apply_effect(effect, event_loop);
                    }
                }
            }
            _ => {}
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // The exit line: how this run ended (the windowed verification's
        // evidence — a menu quit, a window close, the frames budget all
        // read differently here).
        if let Some(host) = &self.host {
            println!(
                "pandemonium client — exiting at the {} (tick {}, {} commands submitted)",
                self.screen.name(),
                host.tick(),
                self.commands_submitted
            );
        } else {
            println!(
                "pandemonium client — exiting at the {} ({} match{} started this run)",
                self.screen.name(),
                self.matches_started,
                if self.matches_started == 1 { "" } else { "es" }
            );
        }
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
        // M9.1: the speed stays frame-delta scaled (a 144 Hz monitor must not
        // pan twice as fast as 60 Hz); the value itself is configurable now
        // (PLAN-M10.2 §3.4 `pan_speed`, default config::DEFAULT_PAN_SPEED).
        let pan = self.settings.pan_speed * dt;
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
        // corners combine (the vector carries both axes). The guards —
        // focus, the settings toggle (§3.4, DEBT-014 repaid: the flag now
        // comes from the settings file and the settings screen flips it),
        // and menus owning the input — live in the pure contribution
        // function the tests pin.
        if let (Some(window), Some((mx, my))) = (&self.window, self.drag_current) {
            let size = window.inner_size();
            if size.width > 0 && size.height > 0 {
                let (scale_x, scale_z) = input::edge_pan_contribution(
                    self.edge_scroll_enabled,
                    self.focused,
                    self.screen.takes_world_input(),
                    (mx, my),
                    (size.width as f64, size.height as f64),
                );
                dx += pan * scale_x;
                dz += pan * scale_z;
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
    // below sees the same stream the windowed client draws from. The menu
    // flow's Player vs AI mode builds exactly this host (A-112).
    let mut host = app::App::build_host(bundle, setup, MatchMode::PlayerVsAi);
    let mut null_renderer = NullRenderer::new();
    // M10.2 Phase 4 (PLAN §4.3): the smoke constructs the REAL sink — the
    // startup probe runs here (on a machine with no audio device: the null
    // arm, the same silent fallback the windowed client takes under
    // Xvfb/CI; the probe's one honest line prints above), and the counters
    // below are the audio evidence. Whatever arm this machine's probe
    // found, the smoke asserts counting, never audible sound.
    let smoke_settings = config::load();
    let mut audio = sound::Sound::new(smoke_settings.master_volume, smoke_settings.muted);
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
        "  feedback wiring: {} attacks flashed on screen, {} refused orders surfaced",
        feedback.hits_seen, feedback.refusals_seen
    );
    // PLAN-M10.2 §4.5: what the audio did — the arm (fallback vs active —
    // this machine's probe result, stated as a fact, never claimed as
    // audible sound) and the cues' three fates.
    println!("  audio wiring: {}", audio.evidence_line());
    assert!(
        refusal_surfaced && feedback.refusals_seen > 0,
        "the invalid order must have surfaced as a refusal cue"
    );
    // The audio wiring ran: the deliberate rejection is itself an event,
    // so a zero count would mean the sink was never fed (counting is the
    // claim — audible sound is never claimed on a machine that cannot
    // hear).
    assert!(
        audio.events_fed() > 0,
        "the audio sink was fed at least the rejection event"
    );
    println!("  content hash:    {:#018x}", bundle.content_hash());
    println!("pandemonium client — smoke PASS (windowed M3 verification requires a display)");
    Ok(())
}

#[cfg(test)]
mod audio_wiring {
    //! PLAN-M10.2 §4.2's wiring pins: the three client-side moments reach
    //! the sink's direct path. The oracle is the null arm's counter (the
    //! engine's `NullAudioSink` counting what reached `on_cue`), so every
    //! test FORCES the null arm first — this machine's probe result is a
    //! fact about the machine, never a test input. One call per fresh app:
    //! the first cue of a kind always passes the rate limiter, so the
    //! oracle count is deterministic.

    use super::*;

    /// An app over the repo content with the null audio arm (the wiring
    /// oracle's precondition). A-002 Part 1: the null arm is CONSTRUCTED,
    /// not forced after the fact — the App never probes this machine's
    /// audio hardware from a unit test.
    fn app() -> App {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        let tree = ContentTree::load_dir(path).expect("the repo content loads");
        let bundle = tree
            .bundle(
                &tree
                    .maps
                    .first()
                    .map(|map| map.id.clone())
                    .expect("the tree lists a map"),
            )
            .expect("the default map bundles");
        App::new(
            tree,
            bundle,
            Some(7),
            None,
            config::Settings::default(),
            // The forced null arm: what the no-device constructor yields, so
            // the oracle counter exists whatever this machine can hear.
            sound::Sound::without_device(1.0, false),
        )
        .expect("the app constructs")
    }

    #[test]
    fn a_submitted_order_acknowledges_on_the_direct_path() {
        let mut app = app();
        // No order leaves a menu (the belt under the routing): no ack.
        app.submit_order(
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(10, 10),
            },
            Some(Vec2Fx::from_ints(10, 10)),
        );
        assert_eq!(app.audio.null_client_cues(), 0, "no host, no submission");
        app.begin_match(MatchMode::PlayerVsAi, 7, 0);
        assert_eq!(app.audio.client_cues(), 0, "begin_match itself is silent");
        // A-120: "acknowledged" = submitted (stamped and handed to the
        // host), not non-rejected — the cue fires at submission.
        app.submit_order(
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(10, 10),
            },
            Some(Vec2Fx::from_ints(10, 10)),
        );
        assert_eq!(
            app.audio.null_client_cues(),
            1,
            "the submission's ack reached the sink"
        );
    }

    #[test]
    fn a_spectators_order_path_stays_silent() {
        // A-113's carry into audio: spectate silences the human order path
        // at the orders layer — a spectator's idle orders are not
        // acknowledgments.
        let mut app = app();
        app.begin_match(MatchMode::AiVsAi, 7, 0);
        app.submit_order(
            CommandKind::Move {
                units: vec![EntityId(1)],
                target: Vec2Fx::from_ints(10, 10),
            },
            Some(Vec2Fx::from_ints(10, 10)),
        );
        assert_eq!(app.audio.client_cues(), 0, "the wiring never fired");
    }

    #[test]
    fn a_selection_membership_change_clicks_but_a_redundant_one_does_not() {
        // A-122: the cue is the *change* — a redundant re-select of the
        // same membership is not a click.
        let mut app = app();
        app.set_selection(vec![EntityId(1)]);
        assert_eq!(app.audio.null_client_cues(), 1);
        assert_eq!(app.selection, vec![EntityId(1)]);
        app.set_selection(vec![EntityId(1)]); // same membership: no cue at all
        assert_eq!(app.audio.client_cues(), 1, "the wiring never fired");
    }

    #[test]
    fn clearing_a_selection_through_the_escape_ladder_clicks() {
        // Escape's ClearSelection rung routes through the same gate (the
        // clear of a non-empty selection is a change).
        let mut app = app();
        app.set_selection(vec![EntityId(2)]);
        assert_eq!(app.audio.null_client_cues(), 1);
    }

    #[test]
    fn activating_a_menu_row_clicks_the_ui_cue() {
        // A-121: every row activation — Enter, click, or the end screen's
        // R — funnels through one core that cues the UI click. Pinned here
        // at the core (menu_activate); the input paths route through it.
        let mut app = app();
        let (next, effect) = app.menu_activate(1, &app.menu_ctx());
        assert!(matches!(next.screen, Screen::Settings { .. }));
        assert_eq!(effect, Effect::None);
        assert_eq!(
            app.audio.null_client_cues(),
            1,
            "the activation reached the sink"
        );
    }
}

#[cfg(test)]
mod mode_pins {
    //! The in-match half of the mode mapping (PLAN-M10.2 §3.3, A-112):
    //! sandbox constructs and ticks with P2's starting force standing
    //! idle, and no mode adds state to the match (the tick-0 hash is the
    //! mode-neutral world's — the controllers are the only difference).

    use super::*;

    fn bundle() -> ContentBundle {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        ContentBundle::load_dir(path).expect("the repo content loads")
    }

    #[test]
    fn a_sandbox_match_constructs_ticks_and_leaves_p2_standing() {
        let bundle = bundle();
        let setup = MatchMode::Sandbox.match_setup(7);
        let mut host = App::build_host(&bundle, setup, MatchMode::Sandbox);
        // Both sides' starting forces exist from tick 0 (22 entities on the
        // Alpha content: two 5-entity armies + the ore nodes + ...).
        let snapshot = host.render_snapshot();
        assert!(snapshot.entities.iter().any(|e| e.owner == PlayerId(1)));
        // 90 ticks of nobody issuing anything: the idler's base stands, the
        // match stays unresolved, no invariant fires (debug asserts run).
        for _ in 0..90 {
            host.advance(std::time::Duration::from_secs_f64(1.0 / 30.0));
        }
        assert!(!host.is_finished(), "an idle sandbox never resolves");
        let snapshot = host.render_snapshot();
        assert!(
            snapshot.entities.iter().any(|e| e.owner == PlayerId(1)),
            "P2's starting force still stands"
        );
    }

    #[test]
    fn same_seed_spectate_hosts_are_a15_identical_and_the_modes_differ_only_in_setup() {
        // The modes differ in the declared controller kinds (which the
        // state hash covers — the players are hashed), so the honest pin is
        // the A15 shape: two same-seed same-mode hosts are bit-identical in
        // hash and command log, controllers included.
        let bundle = bundle();
        let run = |mode: MatchMode| -> (u64, u64, Vec<String>) {
            let mut host = App::build_host(&bundle, mode.match_setup(7), mode);
            let opening = host.state_hash();
            for _ in 0..30 {
                host.advance(std::time::Duration::from_secs_f64(1.0 / 30.0));
            }
            let log = host
                .log()
                .iter()
                .map(|entry| format!("{:?}", entry.kind))
                .collect();
            (opening, host.state_hash(), log)
        };
        let first = run(MatchMode::AiVsAi);
        let second = run(MatchMode::AiVsAi);
        assert_eq!(
            first, second,
            "same-seed spectate hosts are bit-identical (hashes + log)"
        );
        // And the modes are not secretly the same match: the declared
        // controllers differ, so the openings differ.
        let player_vs_ai = run(MatchMode::PlayerVsAi);
        let sandbox = run(MatchMode::Sandbox);
        assert_ne!(
            first.0, player_vs_ai.0,
            "spectate differs from Player vs AI"
        );
        assert_ne!(
            player_vs_ai.0, sandbox.0,
            "sandbox differs from Player vs AI"
        );
    }
}

#[cfg(test)]
mod mission_wiring {
    //! A-011 (ADR-0201 Phase 1): the mission wiring pins — the picker's
    //! Start resolves through the engine mission seam, the mission-start
    //! cutscene opens over a paused host, any key ends the scene and
    //! unpauses, restart rebuilds the same mission, and the mission state
    //! clears on mission-less starts. The oracle is the App itself over the
    //! repo content (which carries the Phase 1 `crossroads_01` mission), the
    //! A-002 null-audio-arm pattern from audio_wiring.

    use super::*;
    use crate::screens::{KeyContext, NavKey};
    use pandemonium_replay::{Checkpoint, ReplayFile};

    /// An app over the repo content with the null audio arm.
    fn app() -> App {
        let path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        let tree = ContentTree::load_dir(path).expect("the repo content loads");
        let bundle = tree
            .bundle(
                &tree
                    .maps
                    .first()
                    .map(|map| map.id.clone())
                    .expect("the tree lists a map"),
            )
            .expect("the default map bundles");
        App::new(
            tree,
            bundle,
            Some(7),
            None,
            config::Settings::default(),
            sound::Sound::without_device(1.0, false),
        )
        .expect("the app constructs")
    }

    /// A pinned context (the machine drives need a known random seed).
    fn ctx() -> KeyContext {
        KeyContext {
            random_seed: 1234,
            seed_prefill: None,
            map_count: 1,
            mission_count: 1,
        }
    }

    #[test]
    fn the_repo_content_carries_the_phase_one_mission() {
        // The wiring below resolves through the tree's mission list; this
        // pins what it finds (the ADR-0201 §9 vertical slice).
        let app = app();
        let mission = app.tree.missions.first().expect("crossroads_01 loads");
        assert_eq!(mission.id, "crossroads_01");
        assert_eq!(mission.display_name, "Crossroads");
        assert_eq!(mission.map, "crossroads_64");
        assert_eq!(mission.objectives.len(), 2);
    }

    #[test]
    fn beginning_a_mission_opens_the_cutscene_over_a_paused_host() {
        // Task 1 + Task 3: begin_mission resolves the mission's map and
        // objectives, builds the host through the engine seam, and opens
        // the mission-start cutscene over the paused host.
        let mut app = app();
        app.begin_mission(0, 7);
        assert_eq!(app.matches_started, 1);
        assert!(matches!(app.screen.screen, Screen::InMatch));
        assert_eq!(app.mission_index, Some(0));
        let mission = app.mission_hud.as_ref().expect("the mission tracker");
        assert_eq!(mission.display_name, "Crossroads");
        assert_eq!(mission.lines.len(), 2);
        // The objective HUD lines derive from the resolved set (the same
        // order the events index).
        assert_eq!(mission.lines[0].text, "Destroy all enemy Command Center");
        assert_eq!(mission.lines[1].text, "Grow to 8 Workers");
        // Task 3: the scene runs over a paused host at tick 0.
        assert!(
            app.cutscene.is_some(),
            "the mission-start cutscene is active"
        );
        let host = app.host.as_ref().expect("the mission host");
        assert!(host.is_paused(), "the cutscene paused the host");
        assert_eq!(host.tick(), 0);
        // The setup is the Player vs AI shape: the human runs the mission
        // player's slot, the scripted AI the opponent.
        assert_eq!(app.setup.players.len(), 2);
        assert_eq!(app.match_mode, MatchMode::PlayerVsAi);
    }

    #[test]
    fn any_key_skips_the_cutscene_unpauses_and_refocuses_the_camera() {
        // Task 3 + Task 5: the skip input ends the scene and unpauses —
        // the wiring path the KeyboardInput guard calls for any key.
        let mut app = app();
        app.begin_mission(0, 7);
        let (anchor_x, anchor_z) = app.start_anchor;
        // Mid-scene the camera is somewhere over the map (the enemy start
        // edge), not yet on the player's anchor.
        assert!(app.cutscene.is_some());
        app.cutscene_input();
        assert!(app.cutscene.is_none(), "the scene ended");
        let host = app.host.as_ref().expect("the mission host");
        assert!(!host.is_paused(), "the skip unpaused the host");
        let target = app.camera.target();
        assert_eq!(
            (target.x, target.z),
            (anchor_x, anchor_z),
            "the camera rests on the player's start (the readable opening view)"
        );
        // A second input is a no-op (no scene, no stray unpause).
        app.cutscene_input();
        assert!(app.cutscene.is_none());
    }

    #[test]
    fn a_mission_matchs_objective_events_feed_the_tracker() {
        // Task 2 + Task 5: a mission host's own events feed the tracker —
        // the exact consumption the draw loop runs per step (FD-9: after
        // the step, from the step's event list). On the Phase 1 content
        // both objectives stay pending in the opening (AUD-003: the
        // optional objective now targets eight workers — above the
        // Legion's starting four — so the mission demands play; the
        // required destroy objective stays pending while the enemy
        // command center stands) — a deterministic event sequence over
        // the real content: no objective events at all in the opening.
        let mut app = app();
        app.begin_mission(0, 7);
        app.cutscene_input(); // unpause so the host steps
        let outcome = app
            .host
            .as_mut()
            .expect("the mission host")
            .advance(std::time::Duration::from_millis(34));
        for event in &outcome.events {
            if let Event::ObjectiveState { index, state } = event {
                app.mission_hud
                    .as_mut()
                    .expect("the tracker")
                    .on_objective_state(*index, *state);
            }
        }
        assert!(
            !outcome
                .events
                .iter()
                .any(|event| matches!(event, Event::ObjectiveState { .. })),
            "no objective resolves in the opening: {:?}",
            outcome.events
        );
        let mission = app.mission_hud.as_ref().expect("the tracker");
        assert_eq!(
            mission.lines[0].state,
            crate::ui::ObjectiveHudState::Pending,
            "the enemy command center still stands"
        );
        assert_eq!(
            mission.lines[1].state,
            crate::ui::ObjectiveHudState::Pending,
            "the starting force is four workers; the objective asks for eight"
        );
        // The match is not over: the required objective is still pending.
        assert!(!app.host.as_ref().expect("the mission host").is_finished());
    }

    #[test]
    fn the_mission_picker_start_resolves_to_the_selected_mission() {
        // Task 1 + Task 5: the machine's StartMission effect, driven through
        // the real activation path (New Match -> Missions -> type a seed ->
        // Start), resolves to the selected mission in the app.
        let state = screens::AppState::at_main_menu();
        let (new_match, _) = state.activate_index(0, &ctx()); // New Match
        let (picker, _) = new_match.activate_index(5, &ctx()); // Missions
        let mut seed_row = picker.clone();
        seed_row.focus_index(1);
        let (typed, _) = seed_row.on_key(NavKey::Digit('7'), &ctx());
        let (next, effect) = typed.activate_index(2, &ctx()); // Start
        assert!(matches!(next.screen, Screen::InMatch));
        assert_eq!(
            effect,
            Effect::StartMission {
                mission_index: 0,
                seed: 7
            }
        );
        // The app resolves the same record begin_mission consumes.
        let mut app = app();
        app.begin_mission(0, 7);
        assert_eq!(app.mission_index, Some(0));
        assert_eq!(
            app.mission_hud.as_ref().expect("the tracker").display_name,
            app.tree.missions[0].display_name
        );
        assert_eq!(app.setup.seed, 7);
    }

    #[test]
    fn a_mission_rematch_rebuilds_the_same_mission_and_other_starts_clear_it() {
        // Restart (A15 over the mission dimension): the same mission, the
        // same seed. A later mission-less start and a drop both clear the
        // mission state (a normal match never shows the mission HUD).
        let mut app = app();
        app.begin_mission(0, 7);
        app.restart_match();
        assert_eq!(app.mission_index, Some(0));
        assert_eq!(
            app.mission_hud.as_ref().expect("the tracker").display_name,
            "Crossroads"
        );
        assert_eq!(app.setup.seed, 7);
        assert!(
            app.cutscene.is_some(),
            "a restarted mission re-opens its scene"
        );
        // A mission-less start clears the mission state.
        app.begin_match(MatchMode::PlayerVsAi, 7, 0);
        assert_eq!(app.mission_index, None);
        assert!(app.mission_hud.is_none());
        // And drop_match clears it too.
        app.begin_mission(0, 7);
        app.drop_match();
        assert_eq!(app.mission_index, None);
        assert!(app.mission_hud.is_none());
        assert!(app.cutscene.is_none());
    }

    #[test]
    fn a_mission_match_recording_carries_the_mission_identity() {
        // AUD-002 (ADR-0201 §7): the --record write path stamps the
        // domain-separated mission id of the mission begin_mission resolved
        // — through the real app: begin_mission -> advance -> the recorded
        // file decodes with `mission_id == tree.mission_id("crossroads_01")`
        // (the key `tools replay-verify`'s third comparison checks), while a
        // plain match's record stays mission-less. The record re-simulates
        // mission-carrying to identical hashes (objectives are derived,
        // unhashed state — the A-066 discipline).
        let tree_path = Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        let tree = ContentTree::load_dir(tree_path).expect("the repo content loads");
        let mission_id = tree.mission_id("crossroads_01");

        // The pure stamp: (tree, Some(0)) is the mission's id, a plain
        // match's None stays None, an out-of-range index is None (defensive).
        assert_eq!(App::mission_replay_id(&tree, Some(0)), Some(mission_id));
        assert_eq!(App::mission_replay_id(&tree, None), None);
        assert_eq!(App::mission_replay_id(&tree, Some(9)), None);

        // The end-to-end record: an app with --record armed, a mission
        // begun and advanced, the recording written and read back.
        let dir = std::env::temp_dir();
        let mission_path = dir.join("pandemonium-aud002-mission-record.pdrp");
        let plain_path = dir.join("pandemonium-aud002-plain-record.pdrp");
        let bundle = tree
            .bundle("crossroads_64")
            .expect("the mission's map bundles");
        let mut app = App::new(
            tree,
            bundle,
            Some(42),
            Some(mission_path.clone()),
            config::Settings::default(),
            sound::Sound::without_device(1.0, false),
        )
        .expect("the app constructs");
        app.begin_mission(0, 42);
        app.cutscene_input(); // unpause so the host steps
        let frame = std::time::Duration::from_millis(34);
        for _ in 0..90 {
            let outcome = app.host.as_mut().expect("the mission host").advance(frame);
            for &(tick, hash) in &outcome.hashes {
                app.checkpoints.push((tick, hash));
            }
        }
        app.write_recording();
        let bytes = std::fs::read(&mission_path).expect("the recording was written");
        let replay = ReplayFile::decode(&bytes).expect("the recording decodes");
        assert_eq!(
            replay.mission_id,
            Some(mission_id),
            "the mission record names its mission"
        );
        // The record re-simulates mission-carrying to identical hashes —
        // the mission seam the recording ran, exercised end to end.
        let objectives = app
            .tree
            .mission_data("crossroads_01", HUMAN)
            .expect("crossroads_01 resolves");
        let setup = app.setup.clone();
        let end_tick = replay.checkpoints.last().expect("the trail").tick;
        let resim = pandemonium_sim::run_command_log_with_mission(
            &app.bundle.world(),
            &setup,
            &replay.commands,
            end_tick,
            Some(objectives),
        );
        assert_eq!(
            resim
                .checkpoints
                .iter()
                .map(|&(tick, hash)| Checkpoint { tick, hash })
                .collect::<Vec<_>>(),
            replay.checkpoints,
            "every checkpoint re-simulates mission-carrying"
        );
        assert_eq!(resim.final_hash, replay.final_hash);

        // The plain-match control in the same app: a mission-less start
        // records no mission id.
        app.begin_match(MatchMode::PlayerVsAi, 42, 0);
        app.record_path = Some(plain_path.clone());
        app.record_written = false;
        app.write_recording();
        let bytes = std::fs::read(&plain_path).expect("the plain recording was written");
        let plain = ReplayFile::decode(&bytes).expect("the plain recording decodes");
        assert_eq!(plain.mission_id, None, "a plain match records no mission");

        // Tidy: the recordings live in the session temp dir only.
        let _ = std::fs::remove_file(&mission_path);
        let _ = std::fs::remove_file(&plain_path);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        content_flag_from_args, frames_budget_arg, headless_requested_from_args, random_seed,
        record_from_args, seed_flag_from_args, version_requested_from_args,
    };

    /// `args(["--seed", "42"])` — the standard helper spelling.
    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn seed_parser_accepts_a_valid_value() {
        assert_eq!(
            seed_flag_from_args(&args(&["pandemonium-client", "--seed", "42"])),
            Some(42)
        );
        assert_eq!(seed_flag_from_args(&args(&["--seed", "0"])), Some(0));
        assert_eq!(
            seed_flag_from_args(&args(&["--seed", "18446744073709551615"])),
            Some(u64::MAX)
        );
    }

    #[test]
    fn seed_parser_is_none_without_the_flag() {
        // PLAN-M10.2 §3.3: the New Match seed field starts *random* when the
        // flag is absent — the plan's own default (the M10.1-era fixed 7 is
        // gone with the boot-time match).
        assert_eq!(seed_flag_from_args(&args(&["pandemonium-client"])), None);
        assert_eq!(seed_flag_from_args(&args(&["--frames", "900"])), None);
    }

    #[test]
    fn seed_parser_is_none_on_a_missing_value() {
        // A missing value means "not asked for" — the field starts random.
        assert_eq!(seed_flag_from_args(&args(&["--seed"])), None);
    }

    #[test]
    fn seed_parser_is_none_on_a_garbage_value() {
        assert_eq!(
            seed_flag_from_args(&args(&["--seed", "not-a-number"])),
            None
        );
        assert_eq!(seed_flag_from_args(&args(&["--seed", "-1"])), None);
        assert_eq!(seed_flag_from_args(&args(&["--seed", ""])), None);
    }

    #[test]
    fn the_random_seed_source_is_fresh_per_call() {
        // A-111: the OS-entropy source (std's RandomState) — two draws in a
        // row differ; 8 draws showing at least two distinct values pins the
        // freshness without a flaky exact-inequality (a single collision is
        // a 2^-64 event, but the set check cannot flake at all).
        let draws: Vec<u64> = (0..8).map(|_| random_seed()).collect();
        assert!(draws.iter().any(|draw| *draw != draws[0]));
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

    #[test]
    fn content_parser_reads_the_path() {
        assert_eq!(
            content_flag_from_args(&args(&["--content", "/opt/pandemonium/content"])),
            Some(std::path::PathBuf::from("/opt/pandemonium/content"))
        );
        assert_eq!(
            content_flag_from_args(&args(&["--content", "content"])),
            Some(std::path::PathBuf::from("content"))
        );
    }

    #[test]
    fn content_parser_is_none_without_the_flag_or_value() {
        assert_eq!(content_flag_from_args(&args(&["pandemonium-client"])), None);
        assert_eq!(content_flag_from_args(&args(&["--seed", "42"])), None);
        // A missing value is NOT a silent default: the override was typed,
        // so the search below proceeds without it and the error (or the
        // found tree) is the user's feedback.
        assert_eq!(content_flag_from_args(&args(&["--content"])), None);
    }

    #[test]
    fn version_flag_is_recognized_in_both_spellings() {
        assert!(version_requested_from_args(&args(&["--version"])));
        assert!(version_requested_from_args(&args(&["-V"])));
        assert!(!version_requested_from_args(&args(&["pandemonium-client"])));
        assert!(!version_requested_from_args(&args(&["--seed", "7"])));
    }

    #[test]
    fn headless_flag_is_recognized() {
        assert!(headless_requested_from_args(&args(&["--headless"])));
        assert!(!headless_requested_from_args(&args(&[
            "pandemonium-client"
        ])));
        assert!(!headless_requested_from_args(&args(&["--seed", "7"])));
    }
}

#[cfg(test)]
mod content_resolution {
    //! A-002 Part 2.1: the runtime content search, pinned as a pure function
    //! (the flag's precedence, the executable-relative candidate, and the
    //! precise listing of every path a failed search tried).

    use super::*;
    use std::path::PathBuf;

    /// A throwaway directory tree (std-only: temp dir + process id).
    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pandemonium-a002-content-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch dir creates");
        dir
    }

    #[test]
    fn the_explicit_flag_is_the_only_candidate_and_wins_when_it_exists() {
        let root = scratch("explicit");
        let named = root.join("my-content");
        std::fs::create_dir_all(&named).expect("named tree creates");
        // The executable-relative candidate exists too — the flag still wins.
        std::fs::create_dir_all(root.join("content")).expect("default tree creates");
        let resolved = resolve_content_dir(Some(&named), &root.join("game.exe"));
        assert_eq!(resolved.as_deref(), Ok(named.as_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_executable_relative_layout_resolves_before_the_dev_path() {
        let root = scratch("exe-relative");
        // The release archive layout: binary and content/ side by side.
        std::fs::create_dir_all(root.join("content")).expect("archive tree creates");
        let exe = root.join("pandemonium-client");
        let resolved = resolve_content_dir(None, &exe).expect("the archive layout resolves");
        assert_eq!(resolved, root.join("content"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_dev_fallback_resolves_when_the_executable_sits_elsewhere() {
        // The cargo-run layout: the binary sits in target/, no content tree
        // next to it, and the compiled-in workspace path answers — exactly
        // what happens on every dev machine and CI checkout. (The full
        // "nothing anywhere" failure can only exist on a machine without the
        // dev tree — the packaged-archive smoke exercises it there.)
        let root = scratch("dev-fallback");
        let exe = root.join("target").join("debug").join("pandemonium-client");
        std::fs::create_dir_all(exe.parent().unwrap()).expect("target dir creates");
        let resolved = resolve_content_dir(None, &exe)
            .expect("the compiled-in dev tree resolves from anywhere in the repo");
        assert_eq!(resolved, PathBuf::from(content_path()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_failed_search_error_names_every_candidate() {
        // The error's shape, pinned on a synthetic candidate list (a real
        // failed search happens only where no dev tree exists — the archive
        // smoke covers that half).
        let candidates = vec![
            PathBuf::from("/opt/install/content"),
            PathBuf::from("/home/dev/repo/content"),
        ];
        let message = content_search_error(&candidates);
        for candidate in &candidates {
            assert!(
                message.contains(&candidate.display().to_string()),
                "the error names {candidate:?}"
            );
        }
        assert!(
            message.contains("--content"),
            "the error teaches the escape hatch"
        );
    }

    #[test]
    fn a_missing_explicit_flag_path_fails_with_only_that_candidate() {
        let root = scratch("explicit-missing");
        // The default tree exists next to the binary, but the flag said
        // otherwise: the player's word is exact, no silent fallback.
        std::fs::create_dir_all(root.join("content")).expect("default tree creates");
        let named = root.join("typed-but-absent");
        let candidates =
            resolve_content_dir(Some(&named), &root.join("game.exe")).expect_err("typo named");
        assert_eq!(candidates, vec![named.clone()], "no silent fallback");
        let _ = std::fs::remove_dir_all(&root);
    }
}
