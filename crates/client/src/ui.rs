//! The command card and bottom-bar panels (plan §11.3's build placement
//! preview, §11.4's command card / production queue display / selection
//! panel): the playability layer the first human pass found missing — the
//! client could move, gather, and attack, but could not train, build,
//! cancel, or rally, so the human could never play the economy the AI
//! plays through the same commands.
//!
//! Everything here is plain presentation data: the card reads the player's
//! own fog-filtered view plus the loaded content, and buttons emit *client
//! intent* — the simulation's command gate remains the sole authority on
//! legality (a refused order surfaces through the existing refusal cue).

use crate::text::{TextAtlas, UiQuad};
use pandemonium_content::ContentBundle;
use pandemonium_sim_api::{
    EntityId, EntityView, GatherSemantics, KindId, MissionData, MissionObjectiveKind,
    MissionObjectiveParameters, ObjectiveState, QueueView, TilePos,
};

/// What pressing a button asks the client to do (FD-2: it becomes a
/// `Command` at the same gate every other order enters through).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ButtonAction {
    /// Enqueue a unit on the selected producer.
    Train {
        /// The producing entity.
        producer: EntityId,
        /// The kind to train.
        unit: KindId,
    },
    /// Arm placement mode: the next left-click places this structure.
    PlaceStructure {
        /// The worker that will build.
        worker: EntityId,
        /// The structure kind.
        structure: KindId,
    },
    /// Cancel one queued item.
    CancelQueueItem {
        /// The producing entity.
        producer: EntityId,
        /// The queue index to cancel.
        index: u16,
    },
}

/// One clickable region of the bottom bar.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Button {
    /// Left edge (pixels).
    pub x: f32,
    /// Top edge (pixels).
    pub y: f32,
    /// Width (pixels).
    pub w: f32,
    /// Height (pixels).
    pub h: f32,
    /// What pressing it does.
    pub action: ButtonAction,
    /// Whether pressing it will do anything (unaffordable/unmet-requirement
    /// buttons still draw, dimmed, so the player sees what is coming).
    pub enabled: bool,
}

impl Button {
    /// Whether the point (window pixels) is inside the button.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// One clickable region of a menu screen (PLAN-M10.2 §3.6): a plain
/// rectangle with the row identity the pure state machine resolves clicks
/// through. The hover/focus visuals are drawn by the menu pass.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MenuButton {
    /// Left edge (pixels).
    pub x: f32,
    /// Top edge (pixels).
    pub y: f32,
    /// Width.
    pub w: f32,
    /// Height.
    pub h: f32,
    /// Which row of the active screen this is (focus-index order).
    pub id: crate::screens::ButtonId,
}

impl MenuButton {
    /// Whether the point (window pixels) is inside the button.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px <= self.x + self.w && py >= self.y && py <= self.y + self.h
    }
}

/// The bottom bar's panel rectangles, computed from the viewport.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BarLayout {
    /// Minimap panel (bottom-left) — painted by the minimap pass.
    pub minimap: [f32; 4],
    /// Selection panel (bottom-center).
    pub selection: [f32; 4],
    /// Command card (bottom-right).
    pub card: [f32; 4],
    /// Production queue strip (sits directly above the card).
    pub queue: [f32; 4],
}

/// The bar's height in pixels (its panels all share it).
pub const BAR_HEIGHT: f32 = 118.0;
/// The queue strip's height (it rides above the bar).
pub const QUEUE_HEIGHT: f32 = 30.0;
/// One command-card button cell's width.
pub const CELL_W: f32 = 96.0;
/// One command-card button cell's height.
pub const CELL_H: f32 = 44.0;

/// Computes the bottom-bar layout for a viewport (margins fixed; panels
/// shrink with the window so the bar stays legible down to ~700px width).
pub fn bar_layout(viewport: (f32, f32)) -> BarLayout {
    let height = viewport.1;
    let bottom = height - BAR_HEIGHT;
    let minimap_w = 176.0_f32.min(viewport.0 * 0.24);
    let card_w = (CELL_W * 3.0 + 8.0).min(viewport.0 * 0.34);
    let minimap = [8.0, bottom, minimap_w, BAR_HEIGHT - 8.0];
    let card = [viewport.0 - card_w - 8.0, bottom, card_w, BAR_HEIGHT - 8.0];
    let selection_left = minimap[0] + minimap[2] + 8.0;
    let selection_w = (card[0] - 8.0 - selection_left).max(0.0);
    let selection = [selection_left, bottom, selection_w, BAR_HEIGHT - 8.0];
    let queue = [card[0], bottom - QUEUE_HEIGHT - 4.0, card_w, QUEUE_HEIGHT];
    BarLayout {
        minimap,
        selection,
        card,
        queue,
    }
}

/// The frame's built UI: the quads to draw and the buttons to hit-test.
#[derive(Default)]
pub struct BuiltUi {
    /// Quads to draw through the UI pass (on top of the world).
    pub quads: Vec<UiQuad>,
    /// Clickable regions (hit-tested before world clicks).
    pub buttons: Vec<Button>,
}

/// The data the command card reads for one frame — all of it the player's
/// own (fog-filtered) view plus loaded content; never simulation internals.
pub struct CardInput<'a> {
    /// The glyph atlas for measuring and laying out text.
    pub atlas: &'a TextAtlas,
    /// The computed bottom-bar layout.
    pub layout: BarLayout,
    /// The player's own entities as of this frame (exact, not blended).
    pub view_entities: &'a [EntityView],
    /// The player's own production queues (producers ascending by id).
    pub production: &'a [QueueView],
    /// The loaded content bundle (defs, faction production lists).
    pub bundle: &'a ContentBundle,
    /// The current selection (own entity ids).
    pub selection: &'a [EntityId],
    /// The player's Ore balance (the Alpha has one resource).
    pub ore: i64,
}

/// Resolves an entity id string to its kind index (the bundle's entities are
/// sorted by id, and kind indices follow that order — plan §10.5).
pub fn kind_of(bundle: &ContentBundle, id: &str) -> Option<KindId> {
    bundle
        .entities
        .iter()
        .position(|def| def.id == id)
        .map(|index| KindId(index as u32))
}

/// The def for a kind index.
pub fn def_of(bundle: &ContentBundle, kind: KindId) -> Option<&pandemonium_content::EntityDef> {
    bundle.entities.get(kind.0 as usize)
}

/// Builds the bottom bar: selection panel, command card, and queue strip.
/// Pure — same inputs, same pixels.
pub fn build_bottom_bar(input: CardInput<'_>) -> BuiltUi {
    let CardInput {
        atlas,
        layout,
        view_entities,
        production,
        bundle,
        selection,
        ore,
    } = input;
    let mut ui = BuiltUi::default();
    let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
    const PAD: f32 = 6.0;

    // The selected entities as views (only own entities are selectable).
    let selected: Vec<&EntityView> = view_entities
        .iter()
        .filter(|entity| selection.contains(&entity.id))
        .collect();

    // ── Selection panel (bottom-center) ──────────────────────────────────
    let sel = layout.selection;
    ui.quads
        .push(atlas.solid_rect(sel[0], sel[1], sel[2], sel[3], [0.0, 0.0, 0.0, 0.5]));
    if selected.is_empty() {
        // No selection: the panel doubles as the controls reminder (the
        // first-time-player bar, plan §13's A14 "unaided" goal).
        let lines = [
            "left-click / drag: select    right-click: move / attack / gather",
            "A + click: attack-move    S: stop    arrows: scroll    Ctrl+wheel: pitch",
            "select a worker to build, a building to train and set rallies",
        ];
        for (index, line) in lines.iter().enumerate() {
            ui.quads.extend(atlas.layout(
                line,
                sel[0] + PAD,
                sel[1] + PAD + atlas.ascent + index as f32 * line_height,
                [0.75, 0.82, 0.9, 0.9],
            ));
        }
    } else if selected.len() == 1 {
        let entity = selected[0];
        let name = def_of(bundle, entity.kind)
            .map(|def| def.display_name.as_str())
            .unwrap_or("unknown");
        ui.quads.extend(atlas.layout(
            name,
            sel[0] + PAD,
            sel[1] + PAD + atlas.ascent,
            [0.95, 0.97, 1.0, 0.95],
        ));
        // The health bar under the name: same color thresholds as the
        // world-space bars (feedback::health_color semantics, restated here
        // to keep this module dependency-free).
        let bar_y = sel[1] + PAD + line_height + 4.0;
        let bar_w = (sel[2] - 2.0 * PAD).min(180.0);
        let fraction = entity.hp_fraction_milli as f32 / 1000.0;
        ui.quads
            .push(atlas.solid_rect(sel[0] + PAD, bar_y, bar_w, 8.0, [0.0, 0.0, 0.0, 0.7]));
        let color = if entity.hp_fraction_milli > 600 {
            [0.25, 0.85, 0.30, 0.95]
        } else if entity.hp_fraction_milli > 300 {
            [0.95, 0.75, 0.20, 0.95]
        } else {
            [0.90, 0.25, 0.20, 0.95]
        };
        ui.quads
            .push(atlas.solid_rect(sel[0] + PAD, bar_y, bar_w * fraction, 8.0, color));
        // M10.2 Phase 2 (PLAN §2.3): the production-queue summary — what a
        // selected producing building is working on: the head item, its
        // progress, and how many items wait behind it. The queue strip
        // above the command card keeps the per-item cancel buttons; the
        // panel answers "is this building doing anything?".
        if let Some(queue) = production.iter().find(|queue| queue.producer == entity.id) {
            let queue_baseline = bar_y + 14.0 + atlas.ascent;
            let line = if queue.items.is_empty() {
                "queue: idle".to_string()
            } else {
                let head = def_of(bundle, queue.items[0].kind)
                    .map(|def| def.display_name.clone())
                    .unwrap_or_else(|| "?".to_string());
                let waiting = queue.items.len() - 1;
                if waiting > 0 {
                    format!(
                        "queue: {} {}%  (+{} waiting)",
                        head,
                        queue.items[0].progress_milli / 10,
                        waiting
                    )
                } else {
                    format!("queue: {} {}%", head, queue.items[0].progress_milli / 10)
                }
            };
            ui.quads.extend(atlas.layout(
                &line,
                sel[0] + PAD,
                queue_baseline,
                [0.7, 0.8, 0.9, 0.9],
            ));
        }
    } else {
        // Multi-selection: a count line plus a per-kind census by display
        // name (data-defined, deterministic — entities arrive kind-ordered
        // by id, and the census sorts by kind index).
        let mut kinds: Vec<(KindId, usize)> = Vec::new();
        for entity in &selected {
            match kinds.iter_mut().find(|(kind, _)| *kind == entity.kind) {
                Some((_, count)) => *count += 1,
                None => kinds.push((entity.kind, 1)),
            }
        }
        let mut line = String::new();
        for (index, (kind, count)) in kinds.iter().enumerate() {
            if index > 0 {
                line.push_str(", ");
            }
            let name = def_of(bundle, *kind)
                .map(|def| def.display_name.as_str())
                .unwrap_or("?");
            line.push_str(&format!("{count} {name}"));
        }
        ui.quads.extend(atlas.layout(
            &line,
            sel[0] + PAD,
            sel[1] + PAD + atlas.ascent,
            [0.95, 0.97, 1.0, 0.95],
        ));
    }

    // ── The subject of the command card: the first selected producer and/or
    // the first selected worker (both may show at once). ──────────────────
    let selected_producer = selected
        .iter()
        .find(|entity| production.iter().any(|queue| queue.producer == entity.id))
        .map(|entity| entity.id);
    let selected_worker = selected
        .iter()
        .find(|entity| {
            def_of(bundle, entity.kind).is_some_and(|def| {
                matches!(
                    def.capability("Build"),
                    Some(pandemonium_content::CapabilityDef::Build)
                )
            })
        })
        .map(|entity| entity.id);

    // ── Command card (bottom-right) ──────────────────────────────────────
    let card = layout.card;
    ui.quads
        .push(atlas.solid_rect(card[0], card[1], card[2], card[3], [0.0, 0.0, 0.0, 0.5]));

    // Train buttons (row 0): the selected producer's faction list.
    if let Some(producer) = selected_producer {
        let row = 0;
        let producer_kind = view_entities
            .iter()
            .find(|entity| entity.id == producer)
            .map(|entity| entity.kind);
        let trainable: Vec<KindId> = producer_kind
            .and_then(|kind| {
                def_of(bundle, kind).and_then(|def| {
                    bundle
                        .faction()
                        .production
                        .iter()
                        .find(|(producer_id, _)| producer_id == &def.id)
                        .map(|(_, list)| list.clone())
                })
            })
            .map(|list| list.iter().filter_map(|id| kind_of(bundle, id)).collect())
            .unwrap_or_default();
        for (column, unit) in trainable.iter().take(3).enumerate() {
            let x = card[0] + PAD + column as f32 * CELL_W;
            let y = card[1] + PAD + row as f32 * CELL_H;
            let Some(def) = def_of(bundle, *unit) else {
                continue;
            };
            // Requirement check against own *visible* entities (an
            // approximation: a still-under-construction requirement also
            // passes here; the gate is the authority and refusals surface).
            let requirements_met = def.requires.iter().all(|required| {
                kind_of(bundle, required).is_some_and(|required_kind| {
                    view_entities
                        .iter()
                        .any(|entity| entity.kind == required_kind)
                })
            });
            let enabled = ore >= def.cost_ore && requirements_met;
            let label = format!("{} {}", def.display_name, def.cost_ore);
            ui.quads.push(atlas.solid_rect(
                x,
                y,
                CELL_W - 8.0,
                CELL_H - 10.0,
                if enabled {
                    [0.12, 0.20, 0.30, 0.9]
                } else {
                    [0.08, 0.08, 0.09, 0.9]
                },
            ));
            ui.quads.extend(atlas.layout(
                &label,
                x + 6.0,
                y + (CELL_H - 10.0) * 0.5 + atlas.ascent * 0.5,
                if enabled {
                    [0.9, 0.95, 1.0, 0.95]
                } else {
                    [0.45, 0.47, 0.5, 0.8]
                },
            ));
            ui.buttons.push(Button {
                x,
                y,
                w: CELL_W - 8.0,
                h: CELL_H - 10.0,
                action: ButtonAction::Train {
                    producer,
                    unit: *unit,
                },
                enabled,
            });
        }
    }
    let row: f32 = if selected_producer.is_some() {
        1.0
    } else {
        0.0
    };

    // Build buttons (the next row, left half): every roster structure the
    // selected worker can place (structures are data; requirements gate).
    // Cells wrap onto a second card row (the card fits two rows of three).
    if let Some(worker) = selected_worker {
        let structures: Vec<KindId> = bundle
            .faction()
            .roster
            .iter()
            .filter_map(|id| kind_of(bundle, id))
            .filter(|kind| def_of(bundle, *kind).is_some_and(|def| def.is_structure()))
            .collect();
        for (cell, structure) in structures.iter().take(6).enumerate() {
            let Some(def) = def_of(bundle, *structure) else {
                continue;
            };
            let column = (cell % 3) as f32;
            let cell_row = row + (cell / 3) as f32;
            let x = card[0] + PAD + column * CELL_W;
            let y = card[1] + PAD + cell_row * CELL_H;
            let enabled = ore >= def.cost_ore;
            let label = format!("+{}", def.display_name);
            ui.quads.push(atlas.solid_rect(
                x,
                y,
                CELL_W - 8.0,
                CELL_H - 10.0,
                if enabled {
                    [0.10, 0.24, 0.14, 0.9]
                } else {
                    [0.08, 0.08, 0.09, 0.9]
                },
            ));
            ui.quads.extend(atlas.layout(
                &label,
                x + 6.0,
                y + (CELL_H - 10.0) * 0.5 + atlas.ascent * 0.5,
                if enabled {
                    [0.8, 1.0, 0.85, 0.95]
                } else {
                    [0.45, 0.47, 0.5, 0.8]
                },
            ));
            ui.buttons.push(Button {
                x,
                y,
                w: CELL_W - 8.0,
                h: CELL_H - 10.0,
                action: ButtonAction::PlaceStructure {
                    worker,
                    structure: *structure,
                },
                enabled,
            });
        }
    }

    // ── Queue strip (above the card): the selected producer's queue with
    // per-item cancel buttons and the head item's progress. ──────────────
    if let Some(producer) = selected_producer {
        let queue = production
            .iter()
            .find(|queue| queue.producer == producer)
            .expect("producer came from the same production list");
        let strip = layout.queue;
        ui.quads.push(atlas.solid_rect(
            strip[0],
            strip[1],
            strip[2],
            strip[3],
            [0.0, 0.0, 0.0, 0.55],
        ));
        let cell_w = 92.0;
        for (index, item) in queue.items.iter().take(3).enumerate() {
            let x = strip[0] + PAD + index as f32 * cell_w;
            let y = strip[1] + 3.0;
            let name = def_of(bundle, item.kind)
                .map(|def| def.display_name.clone())
                .unwrap_or_else(|| "?".to_string());
            let label = if index == 0 {
                format!("{} {}%", name, item.progress_milli / 10)
            } else {
                name
            };
            // A red X tile signals cancel; the whole cell is the button.
            ui.quads.push(atlas.solid_rect(
                x,
                y,
                cell_w - 8.0,
                strip[3] - 6.0,
                [0.10, 0.10, 0.14, 0.9],
            ));
            ui.quads.extend(atlas.layout(
                &label,
                x + 4.0,
                y + (strip[3] - 6.0) * 0.5 + atlas.ascent * 0.45,
                [0.9, 0.9, 0.95, 0.95],
            ));
            ui.quads.extend(atlas.layout(
                "x",
                x + cell_w - 20.0,
                y + (strip[3] - 6.0) * 0.5 + atlas.ascent * 0.45,
                [1.0, 0.45, 0.4, 0.95],
            ));
            ui.buttons.push(Button {
                x,
                y,
                w: cell_w - 8.0,
                h: strip[3] - 6.0,
                action: ButtonAction::CancelQueueItem {
                    producer,
                    index: index as u16,
                },
                enabled: true,
            });
        }
        // The rally marker line (data-defined; SetRally is right-click).
        if queue.rally.is_some() {
            ui.quads.extend(atlas.layout(
                "rally set",
                strip[0] + PAD + 3.0 * cell_w,
                strip[1] + strip[3] * 0.6 + atlas.ascent * 0.4,
                [0.6, 0.95, 0.7, 0.9],
            ));
        }
    }

    ui
}

/// Client-side placement preview (plan §11.3's "legality preview"): bounds,
/// buildable terrain, and static bodies *the player can see*. Approximate by
/// design — the gate re-checks everything, including movers standing in the
/// footprint, and a wrong guess surfaces as a refusal cue.
pub fn placement_is_likely_legal(
    bundle: &ContentBundle,
    view_entities: &[EntityView],
    structure: KindId,
    at: TilePos,
) -> bool {
    let Some((w, h)) = def_of(bundle, structure).and_then(|def| def.footprint()) else {
        return false;
    };
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            let x = at.x + dx;
            let y = at.y + dy;
            if x < 0 || y < 0 || x >= bundle.map.width as i32 || y >= bundle.map.height as i32 {
                return false;
            }
            if !bundle.map.buildable(x, y) {
                return false;
            }
            // Static bodies the player can see claim their footprint tiles.
            for entity in view_entities {
                let Some(def) = def_of(bundle, entity.kind) else {
                    continue;
                };
                let Some((ew, eh)) = def.footprint() else {
                    continue;
                };
                let ex = entity.pos.x.floor_int();
                let ey = entity.pos.y.floor_int();
                if x >= ex && x < ex + ew as i32 && y >= ey && y < ey + eh as i32 {
                    return false;
                }
            }
        }
    }
    true
}

/// The hover tooltip's owner tag (PLAN-M10.2 §2.3): whose thing the cursor
/// rests on. Pure label logic — the display name itself comes from the
/// bundle (`def_of`).
pub fn owner_tag(
    owner: pandemonium_sim_api::PlayerId,
    human: pandemonium_sim_api::PlayerId,
) -> &'static str {
    if owner == human {
        "yours"
    } else if owner == pandemonium_sim_api::PlayerId::NEUTRAL {
        "neutral"
    } else {
        "enemy"
    }
}

/// Whether a window-pixel point sits over one of the bottom-bar panels
/// (selection, card, queue strip, minimap). The tooltip suppresses there:
/// over the UI the cursor means UI, not world.
pub fn cursor_over_bottom_bar(layout: &BarLayout, px: f32, py: f32) -> bool {
    let over = |rect: &[f32; 4]| {
        px >= rect[0] && py >= rect[1] && px <= rect[0] + rect[2] && py <= rect[1] + rect[3]
    };
    over(&layout.selection) || over(&layout.card) || over(&layout.queue) || over(&layout.minimap)
}

/// The hover name tooltip (PLAN-M10.2 §2.3): a small dark panel near the
/// cursor carrying the entity's display name and its owner tag. Clamped
/// into the viewport and flipped to the left of the cursor near the right
/// edge. Pure layout over the atlas metrics — unit-testable without a GPU.
pub fn tooltip_quads(
    atlas: &TextAtlas,
    name: &str,
    tag: &str,
    cursor_px: (f32, f32),
    viewport: (f32, f32),
) -> Vec<UiQuad> {
    let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
    let pad = 5.0;
    let name_w = atlas.measure(name);
    let tag_w = atlas.measure(tag);
    let panel_w = name_w.max(tag_w) + 2.0 * pad;
    let panel_h = 2.0 * line_height + 2.0 * pad;
    // Right of the cursor by default; flip left near the right edge; clamp
    // vertically into the viewport.
    let mut x = cursor_px.0 + 14.0;
    if x + panel_w > viewport.0 {
        x = cursor_px.0 - 14.0 - panel_w;
    }
    x = x.clamp(0.0, (viewport.0 - panel_w).max(0.0));
    let y = (cursor_px.1 - panel_h - 8.0).clamp(0.0, (viewport.1 - panel_h).max(0.0));
    let mut quads = vec![atlas.solid_rect(x, y, panel_w, panel_h, [0.02, 0.04, 0.07, 0.82])];
    quads.extend(atlas.layout(
        name,
        x + pad,
        y + pad + atlas.ascent,
        [0.95, 0.97, 1.0, 0.95],
    ));
    quads.extend(atlas.layout(
        tag,
        x + pad,
        y + pad + line_height + atlas.ascent,
        [0.65, 0.75, 0.85, 0.9],
    ));
    quads
}

// ── The mission HUD (A-011, ADR-0201 Phase 1 Task 2) ────────────────────
//
// Presentation-only state fed by `Event::ObjectiveState` after the step
// (FD-9: the client never polls sim internals — the tracker consumes the
// step's events, exactly like the audio and feedback layers). A mission-less
// match carries no `MissionHud`, so the existing HUD is unchanged.

/// How one mission objective reads on the HUD (ADR-0201 §2): pending until
/// the step's `Event::ObjectiveState` resolves it. The tracker is monotonic
/// per objective — once resolved, a later event for the same index never
/// reverts it (the sim fires each exactly once; the guard is defense).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectiveHudState {
    /// Not yet resolved.
    Pending,
    /// The success condition is met.
    Completed,
    /// The failure condition is met.
    Failed,
}

/// One mission objective's display line: the ASCII text, whether it is
/// optional (never gates the match — it feeds this HUD and the end screen),
/// and its live state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObjectiveLine {
    /// The objective's display text (ASCII — the embedded font subset).
    pub text: String,
    /// Whether the objective is optional.
    pub optional: bool,
    /// The live state.
    pub state: ObjectiveHudState,
}

/// The live mission's presentation state (ADR-0201 Phase 1): the mission's
/// display name plus the ordered objective lines — the indices match the
/// `Event::ObjectiveState` indices (the mission's objective list order).
/// One tracker feeds both the in-match HUD and the end-screen mission text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionHud {
    /// The mission's display name (ASCII).
    pub display_name: String,
    /// The ordered objective lines.
    pub lines: Vec<ObjectiveLine>,
}

impl MissionHud {
    /// Builds the tracker with every objective pending (the mission starts).
    pub fn new(display_name: String, lines: Vec<ObjectiveLine>) -> Self {
        Self {
            display_name,
            lines,
        }
    }

    /// Consumes one `Event::ObjectiveState` (FD-9: called after the step,
    /// from the step's event list). Out-of-range indices are ignored (the
    /// mission's own events always name a live index; the guard is defense),
    /// and a resolved objective never reverts.
    pub fn on_objective_state(&mut self, index: u32, state: ObjectiveState) {
        let Some(line) = self.lines.get_mut(index as usize) else {
            return;
        };
        if line.state != ObjectiveHudState::Pending {
            return; // resolutions are monotonic
        }
        line.state = match state {
            ObjectiveState::Completed => ObjectiveHudState::Completed,
            ObjectiveState::Failed => ObjectiveHudState::Failed,
        };
    }

    /// The end screen's rows (A-011 Task 4): each objective's display text
    /// beside its final state — `PENDING` for objectives the match's
    /// resolution left unresolved (an optional objective the player never
    /// finished, or a mission ended by the elimination rule).
    pub fn end_pairs(&self) -> Vec<(String, String)> {
        self.lines
            .iter()
            .map(|line| (line.display_text(), line.state.end_label().to_string()))
            .collect()
    }

    /// The mission-start cutscene's one-line intro (A-011 Task 3): the
    /// objective summary, semicolon-joined — "Objectives: ..." (ASCII).
    pub fn intro_line(&self) -> String {
        let joined = self
            .lines
            .iter()
            .map(|line| line.display_text())
            .collect::<Vec<_>>()
            .join("; ");
        format!("Objectives: {joined}")
    }
}

impl ObjectiveLine {
    /// The line's display text with the optional marker appended.
    pub fn display_text(&self) -> String {
        if self.optional {
            format!("{} (optional)", self.text)
        } else {
            self.text.clone()
        }
    }
}

impl ObjectiveHudState {
    /// The state's checkbox marker on the HUD line (ASCII).
    fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::Completed => "[x]",
            Self::Failed => "[!]",
        }
    }

    /// The state's end-screen label (ASCII).
    fn end_label(self) -> &'static str {
        match self {
            Self::Pending => "PENDING",
            Self::Completed => "COMPLETED",
            Self::Failed => "FAILED",
        }
    }
}

/// Builds a mission's objective display lines from the resolved objective
/// set (ADR-0201 §2a) — the same `MissionData` the simulation receives
/// through the engine seam, so the HUD's indices are the event indices by
/// construction. Kind ids resolve to display names through the bundle's
/// entity order (the exact order the content seam resolved them by). All
/// text is ASCII (the embedded font subset, ADR-0201 §5).
pub fn objective_lines(mission: &MissionData, bundle: &ContentBundle) -> Vec<ObjectiveLine> {
    mission
        .objectives
        .iter()
        .map(|objective| {
            let text = match (&objective.kind, &objective.parameters) {
                (
                    MissionObjectiveKind::Destroy,
                    MissionObjectiveParameters::Destroy { target_kind: None },
                ) => "Destroy all enemy structures".to_string(),
                (
                    MissionObjectiveKind::Destroy,
                    MissionObjectiveParameters::Destroy {
                        target_kind: Some(kind),
                    },
                ) => format!("Destroy all enemy {}", kind_display(bundle, *kind)),
                (
                    MissionObjectiveKind::BuildTrain,
                    MissionObjectiveParameters::BuildTrain { kind, count },
                ) => format!(
                    "Grow to {count} {}",
                    kind_display_plural(bundle, *kind, *count)
                ),
                // B-017: the survive window, in ticks — the integer the
                // mission authored and the evaluator counts (FD-5; the HUD
                // mirrors the data, never converts it).
                (MissionObjectiveKind::Survive, MissionObjectiveParameters::Survive { ticks }) => {
                    format!("Survive {ticks} ticks")
                }
                // B-017: the protected kind, from the mission player's side
                // of the line ("your") — the fail-only constraint reads as a
                // guard order, the same voice as the destroy lines.
                (MissionObjectiveKind::Protect, MissionObjectiveParameters::Protect { kind }) => {
                    format!("Protect your {}", kind_display(bundle, *kind))
                }
                // B-018: the reach area — shape-neutral (the HUD mirrors the
                // data, never renders the geometry; the mission's world
                // presentation marks the ground). Both shapes read as one
                // movement order.
                (MissionObjectiveKind::ReachArea, MissionObjectiveParameters::ReachArea { .. }) => {
                    "Reach the marked area".to_string()
                }
                // B-018: the gather amount in the authored integer (FD-5; the
                // HUD mirrors the data) — the semantics chooses the verb:
                // cumulative reads as a haul ("Gather 400 ore"), balance as
                // a stockpile to hold ("Hold 400 ore").
                (
                    MissionObjectiveKind::Gather,
                    MissionObjectiveParameters::Gather {
                        amount, semantics, ..
                    },
                ) => match semantics {
                    GatherSemantics::Cumulative => format!("Gather {amount} ore"),
                    GatherSemantics::Balance => format!("Hold {amount} ore"),
                },
                // The content seam rejects kind/parameters mismatches loudly
                // (the loader guarantees agreement); the defensive arm keeps
                // an impossible shape readable rather than panicking the HUD.
                _ => "Objective".to_string(),
            };
            ObjectiveLine {
                text,
                optional: objective.optional,
                state: ObjectiveHudState::Pending,
            }
        })
        .collect()
}

/// One kind id's display name through the bundle's entity order (the same
/// order `ContentTree::mission_data` resolved the ids by); an unknown
/// id falls back to its index, never a panic.
fn kind_display(bundle: &ContentBundle, kind: KindId) -> String {
    bundle
        .entities
        .get(kind.0 as usize)
        .map(|def| def.display_name.clone())
        .unwrap_or_else(|| format!("kind {}", kind.0))
}

/// The display name pluralized for a count (AUD-003): the BuildTrain line
/// communicates an existence target ("grow to N"), so the noun reads as a
/// quantity — "Grow to 8 Workers". The naive plural (append "s" unless the
/// name already ends in one) is presentation-only and covers the authored
/// kind names; a singular stays singular at count 1.
fn kind_display_plural(bundle: &ContentBundle, kind: KindId, count: u32) -> String {
    let name = kind_display(bundle, kind);
    if count == 1 || name.ends_with('s') {
        name
    } else {
        format!("{name}s")
    }
}

/// The in-match objective HUD block (Task 2): a "MISSION <name>" header plus
/// one line per objective — the state marker, the display text — panel-backed
/// at the given top edge, directly under the notice lines. Returns the quads
/// and the block's height (the caller advances its layout cursor). A
/// mission-less match never calls this (the existing HUD is unchanged).
pub fn objective_hud_quads(
    atlas: &TextAtlas,
    mission: &MissionHud,
    top: f32,
) -> (Vec<UiQuad>, f32) {
    /// Padding inside the panel.
    const PAD: f32 = 6.0;
    let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
    let header = format!("MISSION {}", mission.display_name);
    let texts: Vec<String> = mission
        .lines
        .iter()
        .map(|line| format!("{} {}", line.state.marker(), line.display_text()))
        .collect();
    let widest = std::iter::once(atlas.measure(&header))
        .chain(texts.iter().map(|text| atlas.measure(text)))
        .fold(0.0f32, f32::max);
    let panel_h = (texts.len() + 1) as f32 * line_height + 2.0 * PAD;
    let mut quads =
        vec![atlas.solid_rect(8.0, top, widest + 2.0 * PAD, panel_h, [0.0, 0.0, 0.0, 0.5])];
    quads.extend(atlas.layout(
        &header,
        8.0 + PAD,
        top + PAD + atlas.ascent,
        [1.0, 0.85, 0.4, 0.95],
    ));
    for (index, text) in texts.iter().enumerate() {
        let color = match mission.lines[index].state {
            ObjectiveHudState::Pending => [0.9, 0.94, 1.0, 0.95],
            ObjectiveHudState::Completed => [0.6, 1.0, 0.6, 1.0],
            ObjectiveHudState::Failed => [1.0, 0.5, 0.5, 1.0],
        };
        let baseline = top + PAD + atlas.ascent + (index + 1) as f32 * line_height;
        quads.extend(atlas.layout(text, 8.0 + PAD, baseline, color));
    }
    (quads, panel_h)
}

// ── The menu screens (PLAN-M10.2 §3.6) ─────────────────────────────────
//
// One builder renders every screen the pure state machine
// (`crate::screens`) navigates: solid panels, bordered buttons, one
// accent for hover/focus (the Phase 2 blue/orange law — nothing new is
// invented). Buttons are plain rectangles with hit rects in focus-index
// order, so the machine's `activate_index(i)` and the pixel geometry agree
// by construction.

/// The blue/orange accent law the menu pass reuses (Phase 2's palette).
const MENU_BLUE: [f32; 3] = crate::render::TEAM_P0;
/// The amber accent (focus/highlight text).
const MENU_AMBER: [f32; 3] = crate::render::TEAM_P1;

/// One row's label/value pair for the menu pass.
struct Row {
    /// The focus index this row occupies (must match the machine's order).
    index: usize,
    /// The id the machine resolves this row to.
    id: crate::screens::ButtonId,
    /// Left-aligned label text (ASCII only — the embedded font subset).
    label: String,
    /// Right-aligned value text.
    value: String,
}

/// The inputs to [`build_menu`].
pub struct MenuInput<'a> {
    /// The text atlas (the shared overlay pass's glyph source).
    pub atlas: &'a TextAtlas,
    /// The window's pixel size.
    pub viewport: (f32, f32),
    /// The state machine (screen + focus).
    pub screen: &'a crate::screens::AppState,
    /// The live settings (the settings screen's values).
    pub settings: &'a crate::config::Settings,
    /// The loaded maps: (id, display name) in list order.
    pub maps: &'a [(String, String)],
    /// The loaded missions: (id, display name) in list order — A-011
    /// (ADR-0201 Phase 1): the mission picker's Mission field reads the
    /// selected mission's display name from here.
    pub missions: &'a [(String, String)],
    /// The end screen's headline/subline when the screen is the end screen.
    pub end_lines: Option<(&'a str, &'a str)>,
    /// The dimming backdrop's strength over a live match (menus without a
    /// host behind them dim harder).
    pub backdrop: [f32; 4],
    /// A-009 task 1: informational rows rendered as plain text between the
    /// title and the buttons (the Controls screen's binding table, sourced
    /// from [`crate::report::CONTROLS_TABLE`]; A-011: the mission end
    /// screen's objective-outcome rows ride the same block). Empty for every
    /// other screen — those screens render buttons only.
    pub info_lines: &'a [(&'a str, &'a str)],
    /// A-009 task 4: the build-identity footer line drawn at the bottom of
    /// the main menu and the pause menu (the `App::version_line()` —
    /// "pandemonium X.Y git Z content HASH"). Empty string for screens
    /// that do not draw it (per the brief: main menu and pause menu only).
    pub version: &'a str,
}

/// What [`build_menu`] produced: the quads to draw and the hit rects.
pub struct BuiltMenu {
    /// The screen-space quads (panel, rows, focus ring).
    pub quads: Vec<UiQuad>,
    /// The clickable rows, in focus-index order.
    pub buttons: Vec<MenuButton>,
}

/// The button rows of one screen, in focus-index order.
fn screen_rows(input: &MenuInput<'_>) -> Vec<Row> {
    use crate::screens::{ButtonId, Screen};
    let value_of = |settings: &crate::config::Settings, row: usize| -> String {
        match row {
            0 => on_off(settings.edge_scroll),
            1 => format!("{:.0}", settings.pan_speed),
            2 => format!("{:.0}", settings.zoom_min),
            3 => format!("{:.0}", settings.zoom_max),
            4 => format!("{}%", (settings.master_volume * 100.0).round() as i32),
            5 => on_off(settings.muted),
            6 => on_off(settings.fullscreen),
            7 => on_off(settings.debug_overlay),
            _ => String::new(),
        }
    };
    match &input.screen.screen {
        Screen::MainMenu => vec![
            row(0, ButtonId::NewMatch, "New Match", String::new()),
            row(1, ButtonId::Settings, "Settings", String::new()),
            // A-009 task 1: the Controls row opens the new screen.
            row(2, ButtonId::Controls, "Controls", String::new()),
            row(3, ButtonId::Quit, "Quit", String::new()),
        ],
        Screen::NewMatch(fields) => {
            let seed = if fields.seed_text.is_empty() {
                "random".to_string()
            } else {
                fields.seed_text.clone()
            };
            let map_name = input
                .maps
                .get(fields.map_index)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| "?".to_string());
            let map_value = if fields.map_count > 1 {
                format!("< {map_name} >")
            } else {
                map_name
            };
            vec![
                row(
                    0,
                    ButtonId::Mode,
                    "Mode",
                    format!("< {} >", fields.mode.label()),
                ),
                row(1, ButtonId::Seed, "Seed", format!("{seed} [Enter: random]")),
                row(2, ButtonId::Map, "Map", map_value),
                row(3, ButtonId::Start, "Start", String::new()),
                row(4, ButtonId::Back, "Back", String::new()),
                // A-011 (ADR-0201 Phase 1): the Missions row opens the
                // mission picker — after Back so every existing row keeps
                // its index (the A-010 Xvfb evidence script walks this
                // screen by index; Mode/Seed/Map/Start stay rows 0-3).
                row(5, ButtonId::Missions, "Missions", String::new()),
            ]
        }
        // A-011 (ADR-0201 Phase 1, Task 1): the mission picker mirrors the
        // New Match screen's shape — the cycling Mission field (the
        // selected mission's display name), the editable seed, Start, and
        // Back to the New Match fields the picker carried.
        Screen::MissionPicker(fields) => {
            let seed = if fields.seed_text.is_empty() {
                "random".to_string()
            } else {
                fields.seed_text.clone()
            };
            let mission_name = input
                .missions
                .get(fields.mission_index)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| "?".to_string());
            let mission_value = if fields.mission_count > 1 {
                format!("< {mission_name} >")
            } else {
                mission_name
            };
            vec![
                row(0, ButtonId::Mission, "Mission", mission_value),
                row(
                    1,
                    ButtonId::MissionSeed,
                    "Seed",
                    format!("{seed} [Enter: random]"),
                ),
                row(2, ButtonId::MissionStart, "Start", String::new()),
                row(3, ButtonId::Back, "Back", String::new()),
            ]
        }
        Screen::Settings { .. } => {
            let labels = [
                "Edge scroll",
                "Pan speed",
                "Zoom min",
                "Zoom max",
                "Master volume",
                "Mute",
                "Fullscreen",
                "Debug overlay",
            ];
            let mut rows: Vec<Row> = labels
                .iter()
                .enumerate()
                .map(|(index, label)| {
                    row(
                        index,
                        ButtonId::Setting(index),
                        label,
                        value_of(input.settings, index),
                    )
                })
                .collect();
            rows.push(row(8, ButtonId::Done, "Done (save)", String::new()));
            rows
        }
        Screen::PauseMenu => vec![
            row(0, ButtonId::Resume, "Resume", String::new()),
            row(1, ButtonId::Settings, "Settings", String::new()),
            // A-009 task 1: the Controls row on the pause menu.
            row(2, ButtonId::Controls, "Controls", String::new()),
            row(3, ButtonId::PauseRestart, "Restart", String::new()),
            // PLAN-M11 §2 phase 5: the Resign row (confirm-gated like
            // Restart and Quit to Menu).
            row(4, ButtonId::PauseResign, "Resign", String::new()),
            row(5, ButtonId::QuitToMenu, "Quit to Menu", String::new()),
        ],
        Screen::EndScreen => vec![
            row(0, ButtonId::Rematch, "Rematch", String::new()),
            row(1, ButtonId::ToMenu, "Main Menu", String::new()),
        ],
        // A-009 task 1: the Controls screen has one button (Back). The
        // binding table is rendered separately as `info_lines` (not rows —
        // they are plain text, not focusable buttons).
        Screen::Controls { .. } => vec![row(0, ButtonId::Back, "Back", String::new())],
        // A-009 task 2: the confirm screen's two rows — Cancel first
        // (the safer default focus), Confirm second. The message rides as
        // the title's subline.
        Screen::Confirm { .. } => vec![
            row(0, ButtonId::ConfirmCancel, "Cancel", String::new()),
            row(1, ButtonId::ConfirmProceed, "Confirm", String::new()),
        ],
        Screen::InMatch => Vec::new(), // the match owns the input; no rows
    }
}

/// A row helper (keeps the focus-index and id in lockstep).
fn row(index: usize, id: crate::screens::ButtonId, label: &str, value: String) -> Row {
    Row {
        index,
        id,
        label: label.to_string(),
        value,
    }
}

/// `ON`/`OFF` for a settings row's bool.
fn on_off(on: bool) -> String {
    if on { "ON" } else { "OFF" }.to_string()
}

/// The settings row hint (shown under the settings screen's panel): mouse
/// and keyboard both adjust, Done saves, Esc leaves without saving.
const SETTINGS_HINT: &str = "click / left-right: adjust    Enter: toggle or Done    Esc: no save";

/// Builds the active screen's menu: the dim backdrop, the panel, the title,
/// and the rows (label left, value right, focus ring on the focused row).
pub fn build_menu(input: MenuInput<'_>) -> BuiltMenu {
    let MenuInput {
        atlas,
        viewport,
        screen,
        settings: _,
        maps: _,
        missions: _,
        end_lines,
        backdrop,
        info_lines,
        version,
    } = input;
    let mut quads: Vec<UiQuad> = Vec::new();
    let mut buttons = Vec::new();
    let rows = screen_rows(&input);
    if rows.is_empty() {
        return BuiltMenu { quads, buttons };
    }

    // The dim backdrop over whatever is behind the menu.
    quads.push(atlas.solid_rect(0.0, 0.0, viewport.0, viewport.1, backdrop));

    let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
    const PAD: f32 = 10.0;
    const ROW_H: f32 = 30.0;
    const ROW_GAP: f32 = 8.0;
    // A-009 task 1: the Controls screen's info rows use a tighter pitch
    // (line_height + a small gap) so the 16-row table fits the smallest
    // supported window (800×600) alongside the title and the Back button.
    const INFO_GAP: f32 = 4.0;
    let panel_w = 440.0_f32.min(viewport.0 * 0.9);
    let row_w = panel_w - 2.0 * PAD;

    // The title (or the end screen's headline) rides above the rows.
    let (title, subline) = match (&screen.screen, end_lines) {
        (_, Some((headline, subline))) => (headline.to_string(), Some(subline.to_string())),
        (crate::screens::Screen::MainMenu, _) => ("PANDEMONIUM".to_string(), None),
        (crate::screens::Screen::NewMatch(_), _) => ("NEW MATCH".to_string(), None),
        // A-011: the mission picker's title.
        (crate::screens::Screen::MissionPicker(_), _) => ("MISSIONS".to_string(), None),
        (crate::screens::Screen::Settings { .. }, _) => ("SETTINGS".to_string(), None),
        (crate::screens::Screen::PauseMenu, _) => ("PAUSED".to_string(), None),
        // A-009 task 1: the Controls screen's title.
        (crate::screens::Screen::Controls { .. }, _) => ("CONTROLS".to_string(), None),
        // A-009 task 2: the confirm screen's title names the pending action;
        // the subline is the standing warning ("the current match will be
        // lost"). ASCII only — the embedded font subset.
        (
            crate::screens::Screen::Confirm {
                action: crate::screens::ConfirmAction::Restart,
            },
            _,
        ) => (
            "RESTART?".to_string(),
            Some("the current match will be lost".to_string()),
        ),
        (
            crate::screens::Screen::Confirm {
                action: crate::screens::ConfirmAction::QuitToMenu,
            },
            _,
        ) => (
            "QUIT TO MENU?".to_string(),
            Some("the current match will be lost".to_string()),
        ),
        // PLAN-M11 §2 phase 5: the Resign confirm.
        (
            crate::screens::Screen::Confirm {
                action: crate::screens::ConfirmAction::Resign,
            },
            _,
        ) => (
            "RESIGN?".to_string(),
            Some("the current match will be ended".to_string()),
        ),
        _ => ("PANDEMONIUM".to_string(), None),
    };

    let hint = matches!(screen.screen, crate::screens::Screen::Settings { .. });
    // A-009 task 4: the version footer draws on the main menu and the
    // pause menu only (per the brief). Other screens pass an empty
    // string and draw nothing.
    let show_version = !version.is_empty()
        && matches!(
            screen.screen,
            crate::screens::Screen::MainMenu | crate::screens::Screen::PauseMenu
        );
    let mut panel_h = PAD + line_height + PAD + ROW_GAP; // title + gap
    if subline.is_some() {
        panel_h += line_height + 4.0;
    }
    // A-009 task 1: the info_lines block sits between the title (or its
    // subline) and the buttons — one text line per (action, binding) pair.
    if !info_lines.is_empty() {
        panel_h += info_lines.len() as f32 * (line_height + INFO_GAP) + ROW_GAP;
    }
    panel_h += rows.len() as f32 * (ROW_H + ROW_GAP);
    if hint {
        panel_h += line_height + PAD; // the hint line
    }
    // A-009 task 4: the version footer adds one line + a small pad.
    if show_version {
        panel_h += line_height + PAD;
    }
    let panel_x = viewport.0 * 0.5 - panel_w * 0.5;
    let panel_y = viewport.1 * 0.5 - panel_h * 0.5;

    // The panel.
    quads.push(atlas.solid_rect(panel_x, panel_y, panel_w, panel_h, [0.03, 0.05, 0.08, 0.96]));
    let border = panel_border_pub(atlas, [panel_x, panel_y, panel_w, panel_h], MENU_BLUE, 1.5);
    quads.extend(border);

    // The title.
    let title_color = match end_lines {
        // The outcome panel's law from M8: victory reads green, defeat red.
        Some((headline, _)) if headline.starts_with("VICTORY") => [0.6, 1.0, 0.6, 1.0],
        Some((headline, _)) if headline.starts_with("DEFEAT") => [1.0, 0.5, 0.5, 1.0],
        _ => [0.9, 0.94, 1.0, 1.0],
    };
    let title_w = atlas.measure(&title);
    quads.extend(atlas.layout(
        &title,
        panel_x + (panel_w - title_w) * 0.5,
        panel_y + PAD + atlas.ascent,
        title_color,
    ));
    let mut cursor_y = panel_y + PAD + line_height + ROW_GAP;
    if let Some(subline) = &subline {
        let sub_w = atlas.measure(subline);
        quads.extend(atlas.layout(
            subline,
            panel_x + (panel_w - sub_w) * 0.5,
            cursor_y + atlas.ascent,
            [1.0, 1.0, 1.0, 0.9],
        ));
        cursor_y += line_height + 4.0;
    }

    // A-009 task 1: the info_lines block (the Controls screen's binding
    // table). One line per (action, binding) pair — action left, binding
    // right, both as plain text (no focus ring, no button — they are not
    // focusable; only the Back button below is).
    if !info_lines.is_empty() {
        for (action, binding) in info_lines {
            quads.extend(atlas.layout(
                action,
                panel_x + PAD + 8.0,
                cursor_y + atlas.ascent,
                [0.78, 0.84, 0.92, 0.85],
            ));
            let value_w = atlas.measure(binding);
            quads.extend(atlas.layout(
                binding,
                panel_x + panel_w - PAD - 8.0 - value_w,
                cursor_y + atlas.ascent,
                [0.62, 0.68, 0.78, 0.85],
            ));
            cursor_y += line_height + INFO_GAP;
        }
        cursor_y += ROW_GAP;
    }

    // The rows.
    for entry in &rows {
        let focused = screen.focus == entry.index;
        let y = cursor_y;
        // The row's plate: brighter when focused (the hover state — hover
        // moves the same focus the keyboard moves, one visual for both).
        let plate = if focused {
            [
                MENU_BLUE[0] * 0.35 + 0.05,
                MENU_BLUE[1] * 0.35 + 0.05,
                MENU_BLUE[2] * 0.35 + 0.08,
                0.95,
            ]
        } else {
            [0.10, 0.13, 0.18, 0.9]
        };
        quads.push(atlas.solid_rect(panel_x + PAD, y, row_w, ROW_H, plate));
        if focused {
            quads.extend(panel_border_pub(
                atlas,
                [panel_x + PAD, y, row_w, ROW_H],
                MENU_BLUE,
                1.5,
            ));
        }
        // Label (left) and value (right).
        quads.extend(atlas.layout(
            &entry.label,
            panel_x + PAD + 8.0,
            y + (ROW_H - line_height) * 0.5 + atlas.ascent,
            [0.88, 0.92, 0.98, 1.0],
        ));
        let value_color = if focused {
            MENU_AMBER
        } else {
            [0.7, 0.76, 0.85]
        };
        if !entry.value.is_empty() {
            let value_w = atlas.measure(&entry.value);
            quads.extend(atlas.layout(
                &entry.value,
                panel_x + panel_w - PAD - 8.0 - value_w,
                y + (ROW_H - line_height) * 0.5 + atlas.ascent,
                [value_color[0], value_color[1], value_color[2], 1.0],
            ));
        }
        buttons.push(MenuButton {
            x: panel_x + PAD,
            y,
            w: row_w,
            h: ROW_H,
            id: entry.id,
        });
        cursor_y += ROW_H + ROW_GAP;
    }

    if hint {
        let hint_w = atlas.measure(SETTINGS_HINT);
        quads.extend(atlas.layout(
            SETTINGS_HINT,
            panel_x + (panel_w - hint_w) * 0.5,
            cursor_y + PAD + atlas.ascent,
            [0.55, 0.6, 0.68, 0.9],
        ));
        cursor_y += line_height + PAD;
    }

    // A-009 task 4: the build-identity footer line. Small, dimmed, centered
    // at the bottom of the panel — "pandemonium X.Y git Z content HASH"
    // (the same string the F11 sidecar and the F10 debug overlay print).
    // Drawn on the main menu and the pause menu only; other screens pass
    // an empty string and the `show_version` guard above excludes them.
    if show_version {
        let version_w = atlas.measure(version);
        quads.extend(atlas.layout(
            version,
            panel_x + (panel_w - version_w) * 0.5,
            cursor_y + atlas.ascent,
            [0.45, 0.5, 0.58, 0.8],
        ));
    }

    BuiltMenu { quads, buttons }
}

/// A thin bordered rect (the menu pass's public helper — the same shape
/// `main.rs`'s `panel_border` draws for the HUD panels).
fn panel_border_pub(atlas: &TextAtlas, rect: [f32; 4], color: [f32; 3], t: f32) -> Vec<UiQuad> {
    let [x, y, w, h] = rect;
    vec![
        atlas.solid_rect(
            x - t,
            y - t,
            w + 2.0 * t,
            t,
            [color[0], color[1], color[2], 0.9],
        ),
        atlas.solid_rect(
            x - t,
            y + h,
            w + 2.0 * t,
            t,
            [color[0], color[1], color[2], 0.9],
        ),
        atlas.solid_rect(x - t, y, t, h, [color[0], color[1], color[2], 0.9]),
        atlas.solid_rect(x + w, y, t, h, [color[0], color[1], color[2], 0.9]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_content::ContentTree;
    use pandemonium_sim_api::Vec2Fx;
    use pandemonium_sim_api::{AreaSpec, MissionObjective};

    fn bundle() -> ContentBundle {
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        ContentBundle::load_dir(path).expect("the repo content loads")
    }

    fn view_entity(id: u64, kind: KindId, x: i32, y: i32) -> EntityView {
        EntityView {
            id: EntityId(id),
            owner: pandemonium_sim_api::PlayerId(0),
            kind,
            pos: Vec2Fx::from_ints(x, y),
            facing: Vec2Fx::ZERO,
            hp_fraction_milli: 1000,
            move_state: pandemonium_sim_api::MoveState::Idle,
        }
    }

    #[test]
    fn kind_indices_match_the_bundle_order() {
        let bundle = bundle();
        let worker = kind_of(&bundle, "worker").expect("worker in content");
        assert_eq!(
            def_of(&bundle, worker).map(|def| def.id.as_str()),
            Some("worker")
        );
    }

    #[test]
    fn the_bar_layout_keeps_panels_inside_the_viewport() {
        for width in [700.0f32, 1280.0, 1920.0] {
            let layout = bar_layout((width, 800.0));
            assert!(layout.minimap[0] >= 0.0 && layout.minimap[1] + layout.minimap[3] <= 800.0);
            assert!(layout.card[0] + layout.card[2] <= width + 0.5);
            assert!(
                layout.selection[0] >= layout.minimap[0] + layout.minimap[2] - 0.5,
                "selection starts at or right of the minimap"
            );
            assert!(
                layout.queue[1] + layout.queue[3] <= layout.card[1],
                "the queue strip rides above the card"
            );
        }
    }

    #[test]
    fn a_command_center_selection_offers_worker_training() {
        let bundle = bundle();
        let atlas = crate::text::TextAtlas::new();
        let layout = bar_layout((1280.0, 800.0));
        let cc_kind = kind_of(&bundle, "command_center").expect("cc");
        let cc = view_entity(10, cc_kind, 8, 8);
        let production = vec![QueueView {
            producer: cc.id,
            items: Vec::new(),
            rally: None,
        }];
        let selection = vec![cc.id];
        let ui = build_bottom_bar(CardInput {
            atlas: &atlas,
            layout,
            view_entities: std::slice::from_ref(&cc),
            production: &production,
            bundle: &bundle,
            selection: &selection,
            ore: 1000,
        });
        // The Legion's CC trains exactly one kind: the worker.
        assert_eq!(ui.buttons.len(), 1);
        assert!(matches!(ui.buttons[0].action, ButtonAction::Train { .. }));
        assert!(ui.buttons[0].enabled, "1000 ore affords a worker");
    }

    #[test]
    fn an_unaffordable_train_button_draws_but_is_disabled() {
        let bundle = bundle();
        let atlas = crate::text::TextAtlas::new();
        let layout = bar_layout((1280.0, 800.0));
        let cc_kind = kind_of(&bundle, "command_center").expect("cc");
        let cc = view_entity(10, cc_kind, 8, 8);
        let production = vec![QueueView {
            producer: cc.id,
            items: Vec::new(),
            rally: None,
        }];
        let ui = build_bottom_bar(CardInput {
            atlas: &atlas,
            layout,
            view_entities: std::slice::from_ref(&cc),
            production: &production,
            bundle: &bundle,
            selection: vec![cc.id].as_slice(),
            ore: 10,
        });
        assert_eq!(ui.buttons.len(), 1);
        assert!(!ui.buttons[0].enabled, "10 ore cannot afford a worker");
        assert!(ui.buttons[0].contains(ui.buttons[0].x + 1.0, ui.buttons[0].y + 1.0));
        assert!(!ui.buttons[0].contains(ui.buttons[0].x - 1.0, ui.buttons[0].y + 1.0));
    }

    #[test]
    fn a_worker_selection_offers_structure_placement() {
        let bundle = bundle();
        let atlas = crate::text::TextAtlas::new();
        let layout = bar_layout((1280.0, 800.0));
        let worker_kind = kind_of(&bundle, "worker").expect("worker");
        let worker = view_entity(11, worker_kind, 12, 12);
        let ui = build_bottom_bar(CardInput {
            atlas: &atlas,
            layout,
            view_entities: std::slice::from_ref(&worker),
            production: &[],
            bundle: &bundle,
            selection: std::slice::from_ref(&worker.id),
            ore: 1000,
        });
        // The Legion roster fields four structures: CC, barracks, depot, turret.
        let places: Vec<&Button> = ui
            .buttons
            .iter()
            .filter(|button| matches!(button.action, ButtonAction::PlaceStructure { .. }))
            .collect();
        assert_eq!(places.len(), 4, "every roster structure gets a button");
        assert!(places.iter().all(|button| button.enabled));
    }

    #[test]
    fn queue_items_get_cancel_buttons_in_order() {
        let bundle = bundle();
        let atlas = crate::text::TextAtlas::new();
        let layout = bar_layout((1280.0, 800.0));
        let cc_kind = kind_of(&bundle, "command_center").expect("cc");
        let cc = view_entity(10, cc_kind, 8, 8);
        let production = vec![QueueView {
            producer: cc.id,
            items: vec![
                pandemonium_sim_api::QueueItemView {
                    kind: kind_of(&bundle, "worker").expect("worker"),
                    progress_milli: 250,
                },
                pandemonium_sim_api::QueueItemView {
                    kind: kind_of(&bundle, "worker").expect("worker"),
                    progress_milli: 0,
                },
            ],
            rally: None,
        }];
        let ui = build_bottom_bar(CardInput {
            atlas: &atlas,
            layout,
            view_entities: std::slice::from_ref(&cc),
            production: &production,
            bundle: &bundle,
            selection: std::slice::from_ref(&cc.id),
            ore: 0,
        });
        let cancels: Vec<u16> = ui
            .buttons
            .iter()
            .filter_map(|button| match button.action {
                ButtonAction::CancelQueueItem { index, .. } => Some(index),
                _ => None,
            })
            .collect();
        assert_eq!(cancels, vec![0, 1], "one cancel button per queued item");
    }

    #[test]
    fn placement_rejects_blocked_or_occupied_ground() {
        let bundle = bundle();
        let cc_kind = kind_of(&bundle, "command_center").expect("cc");
        // The repo map's starts are on buildable ground; park a CC on tiles
        // (4,4).. and try to place a barracks (3x3) over it.
        let entities = vec![view_entity(10, cc_kind, 4, 4)];
        let structure = kind_of(&bundle, "barracks").expect("barracks");
        assert!(!placement_is_likely_legal(
            &bundle,
            &entities,
            structure,
            TilePos { x: 4, y: 4 }
        ));
        // Open ground near the start is legal (the repo map's start area is
        // buildable ground).
        assert!(placement_is_likely_legal(
            &bundle,
            &entities,
            structure,
            TilePos { x: 10, y: 12 }
        ));
        // Out of bounds is illegal.
        assert!(!placement_is_likely_legal(
            &bundle,
            &entities,
            structure,
            TilePos {
                x: bundle.map.width as i32 - 1,
                y: 5
            }
        ));
    }

    #[test]
    fn the_owner_tag_names_the_three_relations() {
        use pandemonium_sim_api::PlayerId;
        let human = PlayerId(0);
        assert_eq!(owner_tag(PlayerId(0), human), "yours");
        assert_eq!(owner_tag(PlayerId(1), human), "enemy");
        assert_eq!(owner_tag(PlayerId::NEUTRAL, human), "neutral");
    }

    #[test]
    fn the_tooltip_panel_hugs_the_cursor_and_flips_at_the_right_edge() {
        let atlas = TextAtlas::new();
        let viewport = (1280.0, 800.0);
        // Mid-screen: the panel sits right of the cursor, above it.
        let quads = tooltip_quads(&atlas, "Worker", "yours", (640.0, 400.0), viewport);
        assert!(quads.len() >= 3, "panel + name line + tag line");
        let panel = quads[0];
        assert!(panel.x > 640.0, "right of the cursor: {}", panel.x);
        assert!(panel.y + panel.h < 400.0, "above the cursor");
        // Near the right edge the panel flips to the left of the cursor.
        let flipped = tooltip_quads(&atlas, "Command Center", "enemy", (1270.0, 400.0), viewport);
        assert!(flipped[0].x + flipped[0].w <= 1280.0, "clamped inside");
        assert!(flipped[0].x < 1270.0, "flipped left of the cursor");
    }

    #[test]
    fn the_cursor_over_the_bottom_bar_check_covers_the_panels() {
        let viewport = (1920.0, 1080.0);
        let layout = bar_layout(viewport);
        let center = |rect: &[f32; 4]| (rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5);
        for rect in [
            &layout.minimap,
            &layout.selection,
            &layout.card,
            &layout.queue,
        ] {
            let (x, y) = center(rect);
            assert!(
                cursor_over_bottom_bar(&layout, x, y),
                "the panel center reads as over-the-bar"
            );
        }
        assert!(
            !cursor_over_bottom_bar(&layout, 960.0, 200.0),
            "mid-screen is world"
        );
    }

    #[test]
    fn the_info_panel_carries_the_production_queue_summary() {
        let bundle = bundle();
        let atlas = crate::text::TextAtlas::new();
        let cc_kind = kind_of(&bundle, "command_center").expect("cc");
        let cc = view_entity(10, cc_kind, 8, 8);
        let production = |items: usize| -> Vec<QueueView> {
            vec![QueueView {
                producer: cc.id,
                items: (0..items)
                    .map(|index| pandemonium_sim_api::QueueItemView {
                        kind: kind_of(&bundle, "worker").expect("worker"),
                        progress_milli: if index == 0 { 450 } else { 0 },
                    })
                    .collect(),
                rally: None,
            }]
        };
        let build = |prod: &Vec<QueueView>| {
            build_bottom_bar(CardInput {
                atlas: &atlas,
                layout: bar_layout((1280.0, 800.0)),
                view_entities: std::slice::from_ref(&cc),
                production: prod,
                bundle: &bundle,
                selection: std::slice::from_ref(&cc.id),
                ore: 0,
            })
        };
        let busy = build(&production(2));
        let idle = build(&production(0));
        // The busy line ("queue: Worker 45%  (+1 waiting)") and the idle
        // line ("queue: idle") are different strings — different glyph
        // quads — and both panels draw a queue line the bare panel lacks.
        assert_ne!(busy.quads.len(), idle.quads.len(), "busy vs idle text");
        let no_queue = build_bottom_bar(CardInput {
            atlas: &atlas,
            layout: bar_layout((1280.0, 800.0)),
            view_entities: std::slice::from_ref(&cc),
            production: &[],
            bundle: &bundle,
            selection: std::slice::from_ref(&cc.id),
            ore: 0,
        });
        assert!(
            busy.quads.len() > no_queue.quads.len(),
            "the queue line is extra text"
        );
        // The cancel buttons live in the strip, untouched by the summary.
        let cancels = busy
            .buttons
            .iter()
            .filter(|button| matches!(button.action, ButtonAction::CancelQueueItem { .. }))
            .count();
        assert_eq!(cancels, 2, "the busy queue keeps its cancel buttons");
    }

    #[test]
    fn the_name_lookup_feeds_the_panel_and_the_tooltip() {
        // PLAN-M10.2 §2.6: the name lookup the info panel and the hover
        // tooltip use resolves every authored kind to its display name.
        let bundle = bundle();
        for (id, display) in [
            ("worker", "Worker"),
            ("rifleman", "Rifleman"),
            ("raider", "Raider"),
            ("guardian", "Guardian"),
            ("command_center", "Command Center"),
            ("barracks", "Barracks"),
            ("supply_depot", "Supply Depot"),
            ("turret", "Turret"),
            ("ore_node", "Ore Node"),
        ] {
            let kind = kind_of(&bundle, id).unwrap_or_else(|| panic!("{id} resolves"));
            let def = def_of(&bundle, kind).unwrap_or_else(|| panic!("{id} has a def"));
            assert_eq!(def.display_name, display, "{id} names itself for the HUD");
        }
        // An unknown kind id has no def: both callers fall back safely.
        assert!(def_of(&bundle, KindId(9999)).is_none());
    }

    // ── the menu pass (PLAN-M10.2 §3.6) ─────────────────────────────────

    /// A process-wide atlas for the menu tests (fontdue rasterizes the
    /// embedded font once).
    static ATLAS: std::sync::OnceLock<TextAtlas> = std::sync::OnceLock::new();

    fn atlas() -> &'static TextAtlas {
        ATLAS.get_or_init(TextAtlas::new)
    }

    /// The shared test fixtures for the menu pass (settings + maps live as
    /// long as the tests do, so the borrowed input has somewhere to point).
    static SETTINGS: std::sync::OnceLock<crate::config::Settings> = std::sync::OnceLock::new();

    static MAPS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

    /// The loaded missions fixture (A-011: the picker's Mission field reads
    /// the selected mission's display name from the list).
    static MISSIONS: std::sync::OnceLock<Vec<(String, String)>> = std::sync::OnceLock::new();

    fn menu_input(screen: &crate::screens::AppState) -> MenuInput<'_> {
        MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen,
            settings: SETTINGS.get_or_init(crate::config::Settings::default),
            maps: MAPS.get_or_init(|| vec![("crossroads".to_string(), "Crossroads".to_string())]),
            missions: MISSIONS
                .get_or_init(|| vec![("crossroads_01".to_string(), "Crossroads".to_string())]),
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version: "",
        }
    }

    #[test]
    fn the_menu_buttons_match_the_machines_focus_indices_on_every_screen() {
        let screens = [
            crate::screens::AppState::at_main_menu(),
            crate::screens::AppState {
                screen: crate::screens::Screen::NewMatch(crate::screens::NewMatchState {
                    mode: crate::screens::MatchMode::Sandbox,
                    seed_text: "42".to_string(),
                    map_index: 0,
                    map_count: 1,
                }),
                focus: 0,
            },
            crate::screens::AppState {
                screen: crate::screens::Screen::Settings {
                    return_to: crate::screens::SettingsReturn::MainMenu,
                },
                focus: 3,
            },
            crate::screens::AppState {
                screen: crate::screens::Screen::PauseMenu,
                focus: 1,
            },
            crate::screens::AppState {
                screen: crate::screens::Screen::EndScreen,
                focus: 1,
            },
        ];
        for state in &screens {
            let input = MenuInput {
                atlas: atlas(),
                viewport: (1280.0, 800.0),
                screen: state,
                settings: &crate::config::Settings::default(),
                maps: &[("crossroads".to_string(), "Crossroads".to_string())],
                missions: &[],
                end_lines: Some(("VICTORY", "You eliminated the enemy.")),
                backdrop: [0.0, 0.0, 0.0, 0.5],
                info_lines: &[],
                version: "",
            };
            let built = build_menu(input);
            let expected: Vec<crate::screens::ButtonId> = (0..state.button_count())
                .map(|index| state.button_at(index).expect("every index resolves"))
                .collect();
            let got: Vec<crate::screens::ButtonId> =
                built.buttons.iter().map(|button| button.id).collect();
            assert_eq!(got, expected, "the screen {:?}", state.screen);
        }
    }

    #[test]
    fn menu_buttons_are_real_rects_that_catch_their_labels() {
        let state = crate::screens::AppState {
            screen: crate::screens::Screen::Settings {
                return_to: crate::screens::SettingsReturn::PauseMenu,
            },
            focus: 0,
        };
        let built = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &state,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version: "",
        });
        assert_eq!(built.buttons.len(), 9);
        for button in &built.buttons {
            assert!(button.w > 0.0 && button.h > 0.0, "a real rect");
            // Inside the viewport, and the center is inside the rect.
            assert!(button.x >= 0.0 && button.y >= 0.0);
            assert!(
                button.contains(button.x + button.w * 0.5, button.y + button.h * 0.5),
                "the center is hittable"
            );
        }
        // The rows do not overlap (one focus per pixel).
        for (i, a) in built.buttons.iter().enumerate() {
            for b in built.buttons.iter().skip(i + 1) {
                let disjoint =
                    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y;
                assert!(disjoint, "rows {a:?} and {b:?} overlap");
            }
        }
    }

    #[test]
    fn the_new_match_screen_renders_the_fields_and_a_random_seed_shows_the_hint() {
        let with_seed = crate::screens::AppState {
            screen: crate::screens::Screen::NewMatch(crate::screens::NewMatchState {
                mode: crate::screens::MatchMode::AiVsAi,
                seed_text: "4242".to_string(),
                map_index: 0,
                map_count: 1,
            }),
            focus: 0,
        };
        let built = build_menu(menu_input(&with_seed));
        // A-011: the Missions row (row 5, after Back) grew the screen to
        // six buttons — the brief's picker entry as a sub-screen of New
        // Match.
        assert_eq!(built.buttons.len(), 6);
        // (The values ride the quads as text; the ids and the rect geometry
        // are the machine's contract — pinned above.)
        let without_seed = crate::screens::AppState {
            screen: crate::screens::Screen::NewMatch(crate::screens::NewMatchState {
                mode: crate::screens::MatchMode::PlayerVsAi,
                seed_text: String::new(),
                map_index: 0,
                map_count: 1,
            }),
            focus: 0,
        };
        let built = build_menu(menu_input(&without_seed));
        assert_eq!(built.buttons.len(), 6);
    }

    #[test]
    fn the_version_footer_renders_on_the_main_menu_and_the_pause_menu_only() {
        // A-009 task 4: the build-identity footer draws on the main menu and
        // the pause menu only. Other screens pass an empty string and draw
        // nothing. The footer is text quads; the screen that draws it has
        // MORE quads than the same screen with an empty version string.
        let version = "pandemonium 0.1.1 git abc123  content 0x9bc18c521107b262";
        let no_version = "";

        // Main menu: the footer adds text quads on top of the no-version
        // baseline.
        let main_menu = crate::screens::AppState::at_main_menu();
        let with_footer = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &main_menu,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version,
        });
        let without_footer = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &main_menu,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version: no_version,
        });
        assert!(
            with_footer.quads.len() > without_footer.quads.len(),
            "the main menu draws the version footer (extra text quads)"
        );

        // Pause menu: same — the footer adds quads.
        let pause = crate::screens::AppState {
            screen: crate::screens::Screen::PauseMenu,
            focus: 0,
        };
        let with_footer = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &pause,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version,
        });
        let without_footer = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &pause,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version: no_version,
        });
        assert!(
            with_footer.quads.len() > without_footer.quads.len(),
            "the pause menu draws the version footer (extra text quads)"
        );

        // Settings screen: even when version is non-empty, the guard
        // excludes it (the brief scopes the footer to main menu and pause
        // menu only). The quad count must NOT grow.
        let settings = crate::screens::AppState {
            screen: crate::screens::Screen::Settings {
                return_to: crate::screens::SettingsReturn::MainMenu,
            },
            focus: 0,
        };
        let with_footer = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &settings,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version,
        });
        let without_footer = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &settings,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version: no_version,
        });
        assert_eq!(
            with_footer.quads.len(),
            without_footer.quads.len(),
            "the settings screen does NOT draw the version footer"
        );
    }

    // ── the mission HUD (A-011, ADR-0201 Phase 1 Tasks 2 and 4) ─────────

    /// The repo content's tree + the Phase 1 mission's bundle (the objective
    /// lines resolve through the same content seam the app uses).
    fn tree_and_mission_bundle() -> (ContentTree, ContentBundle) {
        let path = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../content"));
        let tree = ContentTree::load_dir(path).expect("the repo content loads");
        let mission = tree
            .missions
            .first()
            .expect("the Phase 1 mission loads")
            .clone();
        let bundle = tree
            .bundle(&mission.map)
            .expect("the mission's map bundles");
        (tree, bundle)
    }

    /// The Phase 1 mission's resolved objective set (the content seam, as
    /// `begin_mission` resolves it).
    fn resolved_mission(tree: &ContentTree) -> MissionData {
        let mission = tree.missions.first().expect("the Phase 1 mission loads");
        tree.mission_data(&mission.id.clone(), pandemonium_sim_api::PlayerId(0))
            .expect("the objectives resolve")
    }

    #[test]
    fn the_objective_tracker_processes_a_sequence_of_objective_state_events() {
        // A-011 Task 5: the HUD state tracker consumes a sequence of
        // `Event::ObjectiveState` payloads — the objective indices are the
        // mission's list order, resolutions are monotonic, and out-of-range
        // indices are ignored (defense; the sim only names live indices).
        let mut hud = MissionHud::new(
            "Crossroads".to_string(),
            vec![
                ObjectiveLine {
                    text: "Destroy all enemy structures".to_string(),
                    optional: false,
                    state: ObjectiveHudState::Pending,
                },
                ObjectiveLine {
                    text: "Grow to 8 Workers".to_string(),
                    optional: true,
                    state: ObjectiveHudState::Pending,
                },
                ObjectiveLine {
                    text: "Hold the line".to_string(),
                    optional: false,
                    state: ObjectiveHudState::Pending,
                },
            ],
        );
        // Every objective starts pending.
        assert!(hud
            .lines
            .iter()
            .all(|line| line.state == ObjectiveHudState::Pending));
        // A sequence of resolutions: index 1 completes, index 2 fails,
        // index 0 completes late.
        hud.on_objective_state(1, ObjectiveState::Completed);
        hud.on_objective_state(2, ObjectiveState::Failed);
        hud.on_objective_state(0, ObjectiveState::Completed);
        assert_eq!(hud.lines[0].state, ObjectiveHudState::Completed);
        assert_eq!(hud.lines[1].state, ObjectiveHudState::Completed);
        assert_eq!(hud.lines[2].state, ObjectiveHudState::Failed);
        // Monotonic: a later event for a resolved objective never reverts
        // it (the sim fires each exactly once; the guard is defense).
        hud.on_objective_state(0, ObjectiveState::Failed);
        assert_eq!(hud.lines[0].state, ObjectiveHudState::Completed);
        // An out-of-range index is ignored, not a panic.
        hud.on_objective_state(9, ObjectiveState::Completed);
        assert_eq!(hud.lines.len(), 3);
    }

    #[test]
    fn objective_lines_build_ascii_text_from_the_resolved_objective_set() {
        // A-011: the HUD lines derive from the same resolved objective set
        // the simulation receives — the line indices are the event indices
        // by construction, and every string is ASCII (the embedded font
        // subset, ADR-0201 §5).
        let (tree, bundle) = tree_and_mission_bundle();
        let resolved = resolved_mission(&tree);
        let lines = objective_lines(&resolved, &bundle);
        assert_eq!(lines.len(), 2);
        // The Phase 1 mission: destroy (required) + grow to 8 workers
        // (optional), in the authored order.
        assert_eq!(lines[0].text, "Destroy all enemy Command Center");
        assert!(!lines[0].optional);
        // AUD-003: the optional objective is an existence target above the
        // starting force — "Grow to 8 Workers", not "Train 4 Worker" (which
        // read as production the evaluator never counts).
        assert_eq!(lines[1].text, "Grow to 8 Workers");
        assert!(lines[1].optional);
        // All display text is ASCII.
        for line in &lines {
            assert!(line.text.is_ascii(), "{} is ASCII", line.text);
            assert!(line.display_text().is_ascii());
        }
        assert!(MissionHud::new("Crossroads".to_string(), lines.clone())
            .intro_line()
            .is_ascii());
    }

    #[test]
    fn the_survive_and_protect_objectives_render_their_hud_lines() {
        // B-017: the two new objective kinds get short HUD lines in the
        // established style — every string ASCII (the embedded font subset,
        // ADR-0201 §5), the line indices the event indices by construction
        // (the same resolved-objective contract). The survive window renders
        // in the authored integer ticks (FD-5: the HUD mirrors the data,
        // never converts it); the protect line names the protected kind from
        // the mission player's side.
        let (tree, bundle) = tree_and_mission_bundle();
        let command_center = tree
            .entities
            .iter()
            .position(|entity| entity.id == "command_center")
            .expect("command_center exists in the tree");
        let worker = tree
            .entities
            .iter()
            .position(|entity| entity.id == "worker")
            .expect("worker exists in the tree");
        let resolved = MissionData {
            player: pandemonium_sim_api::PlayerId(0),
            objectives: vec![
                MissionObjective {
                    kind: MissionObjectiveKind::Survive,
                    parameters: MissionObjectiveParameters::Survive { ticks: 300 },
                    optional: false,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::Protect,
                    parameters: MissionObjectiveParameters::Protect {
                        kind: KindId(worker as u32),
                    },
                    optional: true,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::Protect,
                    parameters: MissionObjectiveParameters::Protect {
                        kind: KindId(command_center as u32),
                    },
                    optional: false,
                },
            ],
            fog_preset: pandemonium_sim_api::FogPreset::Default,
            ai_settings: pandemonium_sim_api::AiSettings::default(),
        };
        let lines = objective_lines(&resolved, &bundle);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].text, "Survive 300 ticks");
        assert!(!lines[0].optional);
        assert_eq!(lines[1].text, "Protect your Worker");
        assert!(lines[1].optional);
        assert_eq!(lines[2].text, "Protect your Command Center");
        assert!(!lines[2].optional);
        for line in &lines {
            assert!(line.text.is_ascii(), "{} is ASCII", line.text);
            assert!(line.display_text().is_ascii());
        }
    }

    #[test]
    fn the_reach_and_gather_objectives_render_their_hud_lines() {
        // B-018: the two new objective kinds get short HUD lines in the
        // established style — every string ASCII (the embedded font
        // subset, ADR-0201 §5), the line indices the event indices by
        // construction (the same resolved-objective contract). The reach
        // line is shape-neutral (the HUD mirrors the data, never renders
        // the geometry); the gather line carries the authored integer
        // amount (FD-5) with the semantics choosing the verb.
        let (_, bundle) = tree_and_mission_bundle();
        let resolved = MissionData {
            player: pandemonium_sim_api::PlayerId(0),
            objectives: vec![
                MissionObjective {
                    kind: MissionObjectiveKind::ReachArea,
                    parameters: MissionObjectiveParameters::ReachArea {
                        area: AreaSpec::Rect {
                            x: 40,
                            y: 40,
                            w: 8,
                            h: 8,
                        },
                    },
                    optional: false,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::ReachArea,
                    parameters: MissionObjectiveParameters::ReachArea {
                        area: AreaSpec::Circle {
                            cx: 24000,
                            cy: 24000,
                            radius: 4000,
                        },
                    },
                    optional: true,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::Gather,
                    parameters: MissionObjectiveParameters::Gather {
                        resource: pandemonium_sim_api::ResourceId(0),
                        amount: 400,
                        semantics: GatherSemantics::Cumulative,
                    },
                    optional: false,
                },
                MissionObjective {
                    kind: MissionObjectiveKind::Gather,
                    parameters: MissionObjectiveParameters::Gather {
                        resource: pandemonium_sim_api::ResourceId(0),
                        amount: 260,
                        semantics: GatherSemantics::Balance,
                    },
                    optional: true,
                },
            ],
            fog_preset: pandemonium_sim_api::FogPreset::Default,
            ai_settings: pandemonium_sim_api::AiSettings::default(),
        };
        let lines = objective_lines(&resolved, &bundle);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].text, "Reach the marked area");
        assert!(!lines[0].optional);
        assert_eq!(lines[1].text, "Reach the marked area");
        assert!(lines[1].optional);
        assert_eq!(lines[2].text, "Gather 400 ore");
        assert!(!lines[2].optional);
        assert_eq!(lines[3].text, "Hold 260 ore");
        assert!(lines[3].optional);
        for line in &lines {
            assert!(line.text.is_ascii(), "{} is ASCII", line.text);
            assert!(line.display_text().is_ascii());
        }
    }

    #[test]
    fn the_end_pairs_carry_each_objectives_final_state() {
        // A-011 Task 4: the end screen's rows — each objective's display
        // text beside its final state, PENDING for unresolved ones.
        let mut hud = MissionHud::new(
            "Crossroads".to_string(),
            vec![
                ObjectiveLine {
                    text: "Destroy all enemy structures".to_string(),
                    optional: false,
                    state: ObjectiveHudState::Pending,
                },
                ObjectiveLine {
                    text: "Grow to 8 Workers".to_string(),
                    optional: true,
                    state: ObjectiveHudState::Pending,
                },
            ],
        );
        assert_eq!(
            hud.end_pairs(),
            vec![
                (
                    "Destroy all enemy structures".to_string(),
                    "PENDING".to_string()
                ),
                (
                    "Grow to 8 Workers (optional)".to_string(),
                    "PENDING".to_string()
                ),
            ]
        );
        hud.on_objective_state(1, ObjectiveState::Completed);
        assert_eq!(
            hud.end_pairs(),
            vec![
                (
                    "Destroy all enemy structures".to_string(),
                    "PENDING".to_string()
                ),
                (
                    "Grow to 8 Workers (optional)".to_string(),
                    "COMPLETED".to_string()
                ),
            ]
        );
        hud.on_objective_state(0, ObjectiveState::Failed);
        assert_eq!(
            hud.end_pairs()[0],
            (
                "Destroy all enemy structures".to_string(),
                "FAILED".to_string()
            )
        );
    }

    #[test]
    fn the_objective_hud_block_draws_one_line_per_objective_plus_the_header() {
        // A-011 Task 2: the block's height grows one line per objective
        // (plus the MISSION header), and the quads land on screen.
        let atlas = TextAtlas::new();
        let hud = MissionHud::new(
            "Crossroads".to_string(),
            vec![
                ObjectiveLine {
                    text: "Destroy all enemy structures".to_string(),
                    optional: false,
                    state: ObjectiveHudState::Pending,
                },
                ObjectiveLine {
                    text: "Grow to 8 Workers".to_string(),
                    optional: true,
                    state: ObjectiveHudState::Pending,
                },
            ],
        );
        let (quads, height) = objective_hud_quads(&atlas, &hud, 50.0);
        assert!(!quads.is_empty());
        let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
        assert_eq!(height, 3.0 * line_height + 12.0); // header + 2 objectives
                                                      // Fewer objectives, less height.
        let one = MissionHud::new(
            "Crossroads".to_string(),
            vec![ObjectiveLine {
                text: "Destroy all enemy structures".to_string(),
                optional: false,
                state: ObjectiveHudState::Pending,
            }],
        );
        let (_, one_height) = objective_hud_quads(&atlas, &one, 50.0);
        assert!(one_height < height);
    }

    #[test]
    fn a_mission_end_screen_renders_the_objective_rows() {
        // A-011 Task 4: the end screen with mission info rows draws more
        // than the same screen without them (the rows are text quads, the
        // same block the Controls table uses — pinned by quad count, the
        // version-footer test's law).
        let end_screen = crate::screens::AppState {
            screen: crate::screens::Screen::EndScreen,
            focus: 0,
        };
        let with_objectives = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &end_screen,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: Some(("VICTORY", "Crossroads")),
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[
                ("Destroy all enemy structures", "COMPLETED"),
                ("Grow to 8 Workers (optional)", "PENDING"),
            ],
            version: "",
        });
        let without_objectives = build_menu(MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen: &end_screen,
            settings: &crate::config::Settings::default(),
            maps: &[],
            missions: &[],
            end_lines: Some(("VICTORY", "You eliminated the enemy.")),
            backdrop: [0.0, 0.0, 0.0, 0.5],
            info_lines: &[],
            version: "",
        });
        assert!(
            with_objectives.quads.len() > without_objectives.quads.len(),
            "the mission end screen draws the objective rows"
        );
    }
}
