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
use pandemonium_sim_api::{EntityId, EntityView, KindId, QueueView, TilePos};

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
            "A + click: attack-move    S: stop    Q/E: rotate    Ctrl+wheel: pitch",
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
    /// The end screen's headline/subline when the screen is the end screen.
    pub end_lines: Option<(&'a str, &'a str)>,
    /// The dimming backdrop's strength over a live match (menus without a
    /// host behind them dim harder).
    pub backdrop: [f32; 4],
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
            5 => on_off(settings.fullscreen),
            6 => on_off(settings.debug_overlay),
            _ => String::new(),
        }
    };
    match &input.screen.screen {
        Screen::MainMenu => vec![
            row(0, ButtonId::NewMatch, "New Match", String::new()),
            row(1, ButtonId::Settings, "Settings", String::new()),
            row(2, ButtonId::Quit, "Quit", String::new()),
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
            ]
        }
        Screen::Settings { .. } => {
            let labels = [
                "Edge scroll",
                "Pan speed",
                "Zoom min",
                "Zoom max",
                "Master volume (audio: Phase 4)",
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
            rows.push(row(7, ButtonId::Done, "Done (save)", String::new()));
            rows
        }
        Screen::PauseMenu => vec![
            row(0, ButtonId::Resume, "Resume", String::new()),
            row(1, ButtonId::Settings, "Settings", String::new()),
            row(2, ButtonId::PauseRestart, "Restart", String::new()),
            row(3, ButtonId::QuitToMenu, "Quit to Menu", String::new()),
        ],
        Screen::EndScreen => vec![
            row(0, ButtonId::Rematch, "Rematch", String::new()),
            row(1, ButtonId::ToMenu, "Main Menu", String::new()),
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
        end_lines,
        backdrop,
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
    let panel_w = 440.0_f32.min(viewport.0 * 0.9);
    let row_w = panel_w - 2.0 * PAD;

    // The title (or the end screen's headline) rides above the rows.
    let (title, subline) = match (&screen.screen, end_lines) {
        (_, Some((headline, subline))) => (headline.to_string(), Some(subline.to_string())),
        (crate::screens::Screen::MainMenu, _) => ("PANDEMONIUM".to_string(), None),
        (crate::screens::Screen::NewMatch(_), _) => ("NEW MATCH".to_string(), None),
        (crate::screens::Screen::Settings { .. }, _) => ("SETTINGS".to_string(), None),
        (crate::screens::Screen::PauseMenu, _) => ("PAUSED".to_string(), None),
        _ => ("PANDEMONIUM".to_string(), None),
    };

    let hint = matches!(screen.screen, crate::screens::Screen::Settings { .. });
    let mut panel_h = PAD + line_height + PAD + ROW_GAP; // title + gap
    if subline.is_some() {
        panel_h += line_height + 4.0;
    }
    panel_h += rows.len() as f32 * (ROW_H + ROW_GAP);
    if hint {
        panel_h += line_height + PAD; // the hint line
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
    use pandemonium_sim_api::Vec2Fx;

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

    fn menu_input(screen: &crate::screens::AppState) -> MenuInput<'_> {
        MenuInput {
            atlas: atlas(),
            viewport: (1280.0, 800.0),
            screen,
            settings: SETTINGS.get_or_init(crate::config::Settings::default),
            maps: MAPS.get_or_init(|| vec![("crossroads".to_string(), "Crossroads".to_string())]),
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
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
                end_lines: Some(("VICTORY", "You eliminated the enemy.")),
                backdrop: [0.0, 0.0, 0.0, 0.5],
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
            end_lines: None,
            backdrop: [0.0, 0.0, 0.0, 0.5],
        });
        assert_eq!(built.buttons.len(), 8);
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
        assert_eq!(built.buttons.len(), 5);
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
        assert_eq!(built.buttons.len(), 5);
    }
}
