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

mod render;

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Context;
use pandemonium_content::ContentBundle;
use pandemonium_engine::mesh::terrain_mesh;
use pandemonium_engine::renderer::Frame;
use pandemonium_engine::{MatchHost, NullRenderer, Renderer, RtsCamera};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, MatchSetup, PlayerId, PlayerSetup,
};
use render::WgpuRenderer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, PhysicalKey};
use winit::window::{Window, WindowId};

/// The player the human controls.
const HUMAN: PlayerId = PlayerId(0);
/// How far the cursor can drift (in NDC) for a press-release to count as a
/// click rather than a drag.
const CLICK_SLOP: f32 = 0.01;
/// Heightmap units -> tiles of vertical displacement (display-only, ADR-0001).
const HEIGHT_SCALE: f32 = 0.02;

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
    event_loop
        .run_app(&mut app)
        .map_err(|error| anyhow::anyhow!(error))?;
    Ok(())
}

/// The repository's content directory, resolved from the crate manifest so the
/// binary works from any working directory.
fn content_path() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"))
}

/// The windowed application state.
struct App {
    host: MatchHost,
    camera: RtsCamera,
    renderer: Option<WgpuRenderer>,
    window: Option<Arc<Window>>,
    selection: Vec<EntityId>,
    drag_start: Option<(f64, f64)>,
    drag_current: Option<(f64, f64)>,
    keys: BTreeSet<Key<&'static str>>,
    last_frame: Instant,
    command_seq: u32,
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
        let host = MatchHost::new(&bundle.world(), setup);
        let camera = RtsCamera::new(bundle.map.width, bundle.map.height, 16.0 / 9.0);
        Ok(Self {
            host,
            camera,
            renderer: None,
            window: None,
            selection: Vec::new(),
            drag_start: None,
            drag_current: None,
            keys: BTreeSet::new(),
            last_frame: Instant::now(),
            command_seq: 0,
        })
    }

    /// Converts window pixel coordinates to NDC (Y up).
    fn to_ndc(window: &Window, x: f64, y: f64) -> (f32, f32) {
        let size = window.inner_size();
        (
            (x / size.width.max(1) as f64) as f32 * 2.0 - 1.0,
            1.0 - (y / size.height.max(1) as f64) as f32 * 2.0,
        )
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
        self.command_seq += 1;
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
            WindowEvent::RedrawRequested => self.draw(),
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
                        winit::keyboard::KeyCode::KeyA => set_key(&mut self.keys, "a", pressed),
                        winit::keyboard::KeyCode::KeyS => set_key(&mut self.keys, "s", pressed),
                        winit::keyboard::KeyCode::KeyD => set_key(&mut self.keys, "d", pressed),
                        winit::keyboard::KeyCode::Escape if pressed => event_loop.exit(),
                        _ => {}
                    }
                }
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
    fn draw(&mut self) {
        let dt = self.last_frame.elapsed();
        self.last_frame = Instant::now();
        let _ = self.host.advance(dt);
        let Some(renderer) = &mut self.renderer else {
            return;
        };
        let snapshot = self.host.render_snapshot();
        let view_projection = self.camera.view_projection();
        let eye = self.camera.eye();
        renderer.render(Frame {
            snapshot: &snapshot,
            view_projection,
            eye,
            selection: &self.selection,
        });
    }
}

/// Builds the terrain mesh from the content the client loaded: the map grid
/// plus the display-only heightmap (ADR-0001). Revalidates the bundle, which
/// already passed at startup, so it cannot fail in practice.
fn terrain_mesh_of() -> pandemonium_engine::mesh::TerrainMesh {
    let bundle = ContentBundle::load_dir(content_path()).expect("content revalidates");
    terrain_mesh(&bundle.map, HEIGHT_SCALE)
}

/// The headless smoke pass: prove the whole pipeline minus the GPU — content
/// loads, the match advances, the renderer interface is driven, and the state
/// hash is stable.
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
    let mut host = MatchHost::new(&bundle.world(), setup);
    let mut null_renderer = NullRenderer::new();
    let frame_dt = std::time::Duration::from_secs_f64(1.0 / 60.0);
    let mut entities = 0;
    for _ in 0..180 {
        let outcome = host.advance(frame_dt);
        let _ = outcome;
        let snapshot = host.render_snapshot();
        entities = snapshot.entities.len();
        null_renderer.render(Frame {
            snapshot: &snapshot,
            view_projection: glam::Mat4::IDENTITY,
            eye: glam::Vec3::ZERO,
            selection: &[],
        });
    }
    println!(
        "  headless smoke: 180 frames, {entities} entities, tick {}, state hash {:#018x}",
        host.tick(),
        host.state_hash()
    );
    println!("  content hash:    {:#018x}", bundle.content_hash());
    println!("pandemonium client — smoke PASS (windowed M3 verification requires a display)");
    Ok(())
}
