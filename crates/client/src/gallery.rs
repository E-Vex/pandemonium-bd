// `gallery/` — no, a single module: the A-020 model gallery (`--gallery`).
// A presentation-only inspection mode for the asset pack: one synthetic
// snapshot with one entity of EVERY content kind on a flat, neutral ground,
// a close camera on the current kind, a slow turntable driven by the wall
// clock (never the simulation — no match exists in this mode, no tick ever
// advances), and the per-kind stats the brief ordered on screen: kind id,
// tier (model / skin / placeholder), triangles, drawn size in meters.
//
// The non-negotiable: it renders through the SAME passes as the game — the
// same `WgpuRenderer::render`, the same terrain/decal/entity/model/UI
// pipelines, the same fallback chain resolved once at renderer construction.
// No separate draw path exists here; this module only composes the inputs
// (a synthetic `RenderSnapshot`, a hand-built flat `TerrainMesh`, the
// camera, and text quads through the same atlas every HUD line uses).
// RenderEntity/RenderSnapshot are the engine's presentation types — the
// simulation is never constructed, never ticked, and never read.

use std::time::Instant;

use glam::Vec3;
use pandemonium_engine::mesh::{TerrainMesh, TerrainVertex};
use pandemonium_engine::renderer::{Frame, HudState};
use pandemonium_engine::{RenderEntity, RenderSnapshot, Renderer, RtsCamera};
use pandemonium_sim_api::{KindId, MoveState, PlayerId};

use crate::render::WgpuRenderer;
use crate::text::UiQuad;

/// One gallery entry: the content kind this slot presents, where its entity
/// stands on the gallery's ground plane, and whether the kind owns its team
/// color (units and structures do; the ore node is neutral by the game's own
/// rule, so the T toggle leaves it gold).
struct GalleryKind {
    name: String,
    kind: KindId,
    pos: [f32; 2],
    neutral: bool,
}

/// The turntable period: one full rotation every twelve seconds — slow
/// enough to read every face, fast enough to see it moves.
const TURNTABLE_PERIOD_SECS: f32 = 12.0;

/// The distance clamp for Up/Down (tiles — the same unit
/// [`RtsCamera::clamp_distance_to`] works in).
const ZOOM_RANGE: (f32, f32) = (4.0, 40.0);

/// The distance a fresh gallery opens at — close, the brief's own word.
const DEFAULT_ZOOM: f32 = 9.0;

/// The gallery's ground plane: centered on the kinds' row's midpoint, wide
/// enough that even the outermost kind (and the widest zoom) still sits on
/// ground, not void.
const GROUND_TILES: u32 = 120;

/// The neutral ground color: a mid grey-green that carries every team color,
/// every skin, and every model albedo without flattering or hiding any of
/// them (the point is comparison, not staging).
const GROUND_COLOR: [f32; 3] = [0.42, 0.44, 0.40];

/// The kinds' row spacing in tiles. Ten: the widest kind (the barracks
/// model, ~2.9 tiles) plus its shadow sits well inside it, and at the
/// gallery's default zoom the visible half-width is ~5 tiles — so the
/// neighbors stay off-screen and the pixel test's diff is exactly the
/// current kind's pixels (a 6-tile row let the next kind's rotating
/// silhouette poke into the frame's edge; the first calibration caught it).
const KIND_SPACING: f32 = 10.0;

/// What a gallery key press did — the caller needs the quit signal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum GalleryAction {
    /// The key changed the gallery's state (kind, zoom, or team).
    Changed,
    /// Nothing this mode owns (the press is ignored).
    Ignored,
    /// Esc — the caller exits the event loop.
    Quit,
}

/// The gallery: presentation-only state over the loaded content. Built from
/// the SAME bundle the renderer was (kind names by content order — the
/// KindId alignment the renderer's tables share, pinned by
/// `every_pack_model_kind_lands_in_its_own_table_slot`).
pub(crate) struct Gallery {
    kinds: Vec<GalleryKind>,
    current: usize,
    zoom: f32,
    team: PlayerId,
    start: Instant,
    /// The hand-built flat ground (installed into the renderer once, before
    /// the first gallery frame — the menu backdrop's terrain never shows).
    ground: TerrainMesh,
    ground_installed: bool,
}

impl Gallery {
    /// One entry per content kind, content order (KindId order); the row
    /// runs west-east along z = 0, and the neutral kinds keep their neutral
    /// owner whatever the T toggle says (the game's own rule for nodes).
    pub(crate) fn new(bundle: &pandemonium_content::ContentBundle) -> Self {
        let kinds: Vec<GalleryKind> = bundle
            .entities
            .iter()
            .enumerate()
            .map(|(index, def)| GalleryKind {
                name: def.id.clone(),
                kind: KindId(index as u32),
                pos: [3.0 + index as f32 * KIND_SPACING, 0.0],
                // The game's own neutrality rule: kinds with a Resource
                // capability (the ore node) never take a team color.
                neutral: def.capability("Resource").is_some(),
            })
            .collect();
        // The row's midpoint: kind 0's x ..= kind 8's x, so the ground's
        // margin is real on both sides (the camera can zoom out to 40
        // tiles and still stand on ground).
        let row_mid = 3.0 + (kinds.len().saturating_sub(1) as f32) * KIND_SPACING / 2.0;
        Self {
            kinds,
            current: 0,
            zoom: DEFAULT_ZOOM,
            team: PlayerId(0),
            start: Instant::now(),
            ground: flat_ground(GROUND_TILES, [row_mid, 0.0]),
            ground_installed: false,
        }
    }

    /// The current kind's gallery position (the camera's focus target).
    pub(crate) fn focus(&self) -> [f32; 2] {
        self.kinds[self.current].pos
    }

    /// One key press, the brief's own map: Left/Right = kind (wrapping),
    /// Up/Down = zoom (clamped), T = team color P0/P1, Esc = quit. Pure
    /// state — the wiring passes winit's physical code and routes by the
    /// returned action.
    pub(crate) fn handle_key(&mut self, key: winit::keyboard::KeyCode) -> GalleryAction {
        use winit::keyboard::KeyCode;
        if self.kinds.is_empty() {
            return GalleryAction::Ignored;
        }
        match key {
            KeyCode::ArrowLeft => {
                self.current = (self.current + self.kinds.len() - 1) % self.kinds.len();
                GalleryAction::Changed
            }
            KeyCode::ArrowRight => {
                self.current = (self.current + 1) % self.kinds.len();
                GalleryAction::Changed
            }
            KeyCode::ArrowUp => {
                self.zoom = (self.zoom - 1.0).max(ZOOM_RANGE.0);
                GalleryAction::Changed
            }
            KeyCode::ArrowDown => {
                self.zoom = (self.zoom + 1.0).min(ZOOM_RANGE.1);
                GalleryAction::Changed
            }
            KeyCode::KeyT => {
                self.team = if self.team == PlayerId(0) {
                    PlayerId(1)
                } else {
                    PlayerId(0)
                };
                GalleryAction::Changed
            }
            KeyCode::Escape => GalleryAction::Quit,
            _ => GalleryAction::Ignored,
        }
    }

    /// The turntable's facing at an elapsed-seconds offset — a full rotation
    /// every [`TURNTABLE_PERIOD_SECS`], counterclockwise seen from above,
    /// starting at east (yaw 0). Pure: the tests pin the start and the
    /// quarter-turn, the frame path feeds it the wall clock, and no
    /// simulation time exists in this mode.
    fn turntable_facing(elapsed: f32) -> Vec3 {
        let theta = std::f32::consts::TAU * elapsed / TURNTABLE_PERIOD_SECS;
        Vec3::new(theta.cos(), 0.0, theta.sin())
    }

    /// The synthetic snapshot: one entity of every kind, ascending ids (the
    /// snapshot's own contract), every entity turning on the same turntable
    /// so the frame is one coherent scene. The neutral kinds stay neutral;
    /// everything else wears the T toggle's player.
    fn snapshot(&self) -> RenderSnapshot {
        let facing = Self::turntable_facing(self.start.elapsed().as_secs_f32());
        let entities = self
            .kinds
            .iter()
            .enumerate()
            .map(|(index, kind)| RenderEntity {
                id: pandemonium_sim_api::EntityId(index as u64),
                owner: if kind.neutral { PlayerId(2) } else { self.team },
                kind: kind.kind,
                pos: Vec3::new(kind.pos[0], 0.0, kind.pos[1]),
                facing,
                hp_fraction_milli: 1000,
                move_state: MoveState::Idle,
            })
            .collect();
        RenderSnapshot { tick: 0, entities }
    }

    /// The on-screen stats lines: kind id, tier, triangles, drawn size in
    /// meters — plus the key legend, so a first-time viewer needs no manual.
    /// ASCII only (the embedded font is an ASCII subset).
    fn stats_lines(&self, presentation: &crate::render::KindPresentation) -> [String; 4] {
        let kind = &self.kinds[self.current];
        let size = presentation.drawn_meters;
        [
            format!(
                "GALLERY {}/{}  {}",
                self.current + 1,
                self.kinds.len(),
                kind.name
            ),
            format!(
                "TIER {}  TRIS {}  SIZE {:.1} x {:.1} x {:.1} m",
                presentation.tier.as_str(),
                presentation.triangles,
                size[0],
                size[1],
                size[2],
            ),
            format!(
                "TEAM {}",
                if self.team == PlayerId(0) { "P0" } else { "P1" }
            ),
            "[Left/Right] kind  [Up/Down] zoom  [T] team  [Esc] quit".to_string(),
        ]
    }

    /// The stats text as UI quads through the same atlas + UI pass every HUD
    /// line uses. Top-left block, line height from the atlas's own metrics.
    fn stats_quads(
        &self,
        atlas: &crate::text::TextAtlas,
        presentation: &crate::render::KindPresentation,
    ) -> Vec<UiQuad> {
        let mut quads = Vec::new();
        let margin = 14.0;
        let line_height = 20.0;
        for (row, line) in self.stats_lines(presentation).into_iter().enumerate() {
            let baseline = margin + line_height * (row as f32 + 1.0);
            // The kind line reads a touch brighter than the legend; the
            // tier line carries the numbers the owner asked to compare.
            let color = match row {
                0 => [1.0, 1.0, 1.0, 1.0],
                1 => [0.85, 0.95, 1.0, 1.0],
                3 => [1.0, 1.0, 1.0, 0.65],
                _ => [1.0, 1.0, 1.0, 0.9],
            };
            quads.extend(atlas.layout(&line, margin, baseline, color));
        }
        quads
    }
}

/// Renders one gallery frame through the game's own render passes: installs
/// the flat ground once, focuses the camera on the current kind at the
/// current zoom, composes the synthetic snapshot + the stats text, and hands
/// the renderer a [`Frame`] — the same call a match frame makes.
pub(crate) fn render_frame(
    gallery: &mut Gallery,
    renderer: &mut WgpuRenderer,
    camera: &mut RtsCamera,
) {
    if !gallery.ground_installed {
        renderer.set_terrain(&gallery.ground);
        gallery.ground_installed = true;
    }
    let [x, z] = gallery.focus();
    camera.focus(x, z, gallery.zoom);
    // The stats describe the CURRENT kind; unknown ids fall back to a
    // placeholder row (never expected with real content — the kind list is
    // the content's own).
    let presentation = renderer
        .presentation_of(gallery.kinds[gallery.current].kind)
        .unwrap_or(crate::render::KindPresentation {
            tier: crate::pack::PresentationTier::Placeholder,
            triangles: 0,
            drawn_meters: [0.0; 3],
        });
    let quads = gallery.stats_quads(renderer.atlas(), &presentation);
    renderer.queue_ui(&quads);
    let snapshot = gallery.snapshot();
    let hud = HudState::default();
    renderer.render(Frame {
        snapshot: &snapshot,
        view_projection: camera.view_projection(),
        eye: camera.eye(),
        selection: &[],
        hud: &hud,
        flashes: &[],
    });
}

/// The gallery's ground: `tiles` x `tiles` flat quads at y = 0 in one
/// uniform neutral color, centered on `center` — no heightmap, no terrain
/// classes, no fog variation, so nothing on screen competes with the kinds
/// being compared. Hand-built (not `terrain_mesh`) on purpose: the gallery
/// is map-free.
fn flat_ground(tiles: u32, center: [f32; 2]) -> TerrainMesh {
    let half = tiles as f32 / 2.0;
    let (cx, cz) = (center[0] - half, center[1] - half);
    let mut vertices = Vec::with_capacity((tiles * tiles * 4) as usize);
    let mut indices = Vec::with_capacity((tiles * tiles * 6) as usize);
    for z in 0..tiles {
        for x in 0..tiles {
            let x0 = x as f32 + cx;
            let z0 = z as f32 + cz;
            let base = vertices.len() as u32;
            // One quad per tile, the same corner order `terrain_mesh` uses
            // (row-major, counterclockwise seen from above at y = 0).
            for (dx, dz) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
                vertices.push(TerrainVertex {
                    position: [x0 + dx, 0.0, z0 + dz],
                    color: GROUND_COLOR,
                    normal: [0.0, 1.0, 0.0],
                    material: 0.0,
                });
            }
            indices.extend_from_slice(&[base, base + 2, base + 1, base + 1, base + 2, base + 3]);
        }
    }
    TerrainMesh { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gallery() -> Gallery {
        let content = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../content");
        let tree = pandemonium_content::ContentTree::load_dir(&content)
            .expect("the real content tree loads");
        let bundle = tree
            .bundle(
                &tree
                    .maps
                    .first()
                    .expect("the content tree lists maps")
                    .id
                    .clone(),
            )
            .expect("the default map bundles");
        Gallery::new(&bundle)
    }

    /// The gallery covers EVERY content kind exactly once, in content order
    /// (KindId order — the alignment the render tables share), one entity
    /// each, with the KindId the renderer's own tables index by.
    #[test]
    fn every_content_kind_appears_once_in_content_order() {
        let gallery = gallery();
        assert_eq!(gallery.kinds.len(), 9, "the alpha content has nine kinds");
        for (index, kind) in gallery.kinds.iter().enumerate() {
            assert_eq!(
                kind.kind,
                KindId(index as u32),
                "content order is KindId order"
            );
        }
        let names: Vec<&str> = gallery.kinds.iter().map(|k| k.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "barracks",
                "command_center",
                "guardian",
                "ore_node",
                "raider",
                "rifleman",
                "supply_depot",
                "turret",
                "worker",
            ]
        );
    }

    /// The snapshot is the snapshot contract's own shape: ascending ids, one
    /// entity per kind, every kind present, no tick that ever advances.
    #[test]
    fn the_snapshot_carries_one_entity_per_kind_in_ascending_id_order() {
        let gallery = gallery();
        let snapshot = gallery.snapshot();
        assert_eq!(snapshot.tick, 0, "presentation-only: no simulation time");
        assert_eq!(snapshot.entities.len(), gallery.kinds.len());
        for (index, entity) in snapshot.entities.iter().enumerate() {
            assert_eq!(entity.id.0, index as u64, "ascending ids");
            assert_eq!(entity.kind, gallery.kinds[index].kind);
        }
        // The ore node is neutral by the game's own rule; everything else
        // wears the toggle's player.
        assert_eq!(
            gallery.kinds.iter().filter(|k| k.neutral).count(),
            1,
            "exactly the ore node is neutral"
        );
        let ore = gallery
            .kinds
            .iter()
            .position(|k| k.name == "ore_node")
            .unwrap();
        assert_eq!(snapshot.entities[ore].owner, PlayerId(2));
        assert_eq!(snapshot.entities[0].owner, gallery.team);
    }

    /// The keys do exactly what the brief's control map says: wrapping kind
    /// cycle, clamped zoom, team toggle, Esc quits, and nothing else fires.
    #[test]
    fn the_keys_cycle_kind_clamp_zoom_toggle_team_and_quit() {
        let mut gallery = gallery();
        assert_eq!(
            gallery.handle_key(winit::keyboard::KeyCode::ArrowRight),
            GalleryAction::Changed
        );
        assert_eq!(gallery.current, 1);
        assert_eq!(
            gallery.handle_key(winit::keyboard::KeyCode::ArrowLeft),
            GalleryAction::Changed
        );
        assert_eq!(gallery.current, 0, "left and right are inverses");
        for _ in 0..9 {
            gallery.handle_key(winit::keyboard::KeyCode::ArrowRight);
        }
        assert_eq!(gallery.current, 0, "the cycle wraps");
        // Zoom: clamped at both ends, steps of one tile.
        for _ in 0..20 {
            gallery.handle_key(winit::keyboard::KeyCode::ArrowUp);
        }
        assert_eq!(gallery.zoom, ZOOM_RANGE.0, "zoom in clamps");
        for _ in 0..60 {
            gallery.handle_key(winit::keyboard::KeyCode::ArrowDown);
        }
        assert_eq!(gallery.zoom, ZOOM_RANGE.1, "zoom out clamps");
        // Team: P0 -> P1 -> P0.
        assert_eq!(gallery.team, PlayerId(0));
        gallery.handle_key(winit::keyboard::KeyCode::KeyT);
        assert_eq!(gallery.team, PlayerId(1));
        gallery.handle_key(winit::keyboard::KeyCode::KeyT);
        assert_eq!(gallery.team, PlayerId(0));
        // Esc quits; an unowned key is ignored.
        assert_eq!(
            gallery.handle_key(winit::keyboard::KeyCode::Escape),
            GalleryAction::Quit
        );
        assert_eq!(
            gallery.handle_key(winit::keyboard::KeyCode::KeyA),
            GalleryAction::Ignored
        );
    }

    /// The turntable starts facing east (yaw 0 — the direction-pinned
    /// convention) and completes a quarter turn in a quarter of the period.
    #[test]
    fn the_turntable_starts_east_and_turns_at_the_named_rate() {
        let start = Gallery::turntable_facing(0.0);
        assert!((start.x - 1.0).abs() < 1e-6 && start.z.abs() < 1e-6);
        let quarter = Gallery::turntable_facing(TURNTABLE_PERIOD_SECS / 4.0);
        assert!(
            quarter.x.abs() < 1e-6 && (quarter.z - 1.0).abs() < 1e-6,
            "a quarter period turns east to north"
        );
        let full = Gallery::turntable_facing(TURNTABLE_PERIOD_SECS);
        assert!((full.x - 1.0).abs() < 1e-6, "a full period returns east");
    }

    /// The gallery's ground: flat, uniform, the exact quad count the
    /// terrain pass expects — and covering every kind's position with a
    /// margin even the widest zoom can see past (the corner kinds stand on
    /// ground, not void).
    #[test]
    fn the_ground_is_flat_uniform_and_covers_the_whole_row() {
        let gallery = gallery();
        let ground = &gallery.ground;
        assert_eq!(
            ground.vertices.len(),
            (GROUND_TILES * GROUND_TILES * 4) as usize
        );
        assert_eq!(
            ground.indices.len(),
            (GROUND_TILES * GROUND_TILES * 6) as usize
        );
        for vertex in &ground.vertices {
            assert_eq!(vertex.position[1], 0.0, "flat");
            assert_eq!(vertex.color, GROUND_COLOR, "uniform");
            assert_eq!(vertex.normal, [0.0, 1.0, 0.0], "all face up");
        }
        for index in &ground.indices {
            assert!(
                (*index as usize) < ground.vertices.len(),
                "indices in bounds"
            );
        }
        // The first capture's lesson: kind 8 at x = 51 stood beyond a
        // 48-tile ground centered on the row's start. The ground is now
        // centered on the row's midpoint and every kind stands on it with
        // margin.
        let mut xs = [f32::MAX, f32::MIN];
        for vertex in &ground.vertices {
            xs[0] = xs[0].min(vertex.position[0]);
            xs[1] = xs[1].max(vertex.position[0]);
        }
        for kind in &gallery.kinds {
            assert!(
                kind.pos[0] > xs[0] + 10.0 && kind.pos[0] < xs[1] - 10.0,
                "kind at x {} must stand on ground ({:?}) with margin",
                kind.pos[0],
                xs
            );
        }
    }

    /// The stats lines name the kind, the tier, the triangles, and the drawn
    /// size in meters — ASCII only (the embedded font is an ASCII subset).
    #[test]
    fn the_stats_lines_name_kind_tier_triangles_and_meters() {
        let gallery = gallery();
        let lines = gallery.stats_lines(&crate::render::KindPresentation {
            tier: crate::pack::PresentationTier::Model,
            triangles: 864,
            drawn_meters: [1.9, 1.8, 1.2],
        });
        assert!(
            lines[0].contains(&gallery.kinds[gallery.current].name),
            "names the current kind: {}",
            lines[0]
        );
        assert!(lines[0].contains("1/9"), "names the position: {}", lines[0]);
        assert!(lines[1].contains("model"), "names the tier: {}", lines[1]);
        assert!(
            lines[1].contains("864"),
            "names the triangles: {}",
            lines[1]
        );
        assert!(
            lines[1].contains("1.9 x 1.8 x 1.2 m"),
            "names the drawn size: {}",
            lines[1]
        );
        assert!(lines[2].contains("P0"));
        assert!(lines[3].contains("[T] team"));
        for line in lines {
            assert!(
                line.is_ascii(),
                "the embedded font is an ASCII subset: {line}"
            );
        }
    }

    /// The kind row: every kind's position matches its slot, and the focus
    /// target follows the selection.
    #[test]
    fn the_row_and_focus_follow_the_current_kind() {
        let mut gallery = gallery();
        assert_eq!(gallery.focus(), [3.0, 0.0]);
        gallery.handle_key(winit::keyboard::KeyCode::ArrowRight);
        assert_eq!(gallery.focus(), [3.0 + KIND_SPACING, 0.0]);
    }
}
