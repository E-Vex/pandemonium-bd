// `input/event.rs` (ADR-0100 part c, the ARCHITECTURE.md refactor map): the
// winit event translation — camera keys, selection (point + box + double
// click), context orders, placement, control groups, and the order
// submission that routes through `orders::resolve_context_order`. These are
// `App` methods living beside the pure input machine in `super` (`mod.rs`,
// the moved `input.rs`) and the order routing in `orders.rs`.
//
// The methods are `pub(crate)` because `main.rs`'s `ApplicationHandler`
// dispatches into them; their bodies moved verbatim from `app/mod.rs`
// (ADR step 7) — visibility aside, no signature or behavior moved.
use std::collections::BTreeSet;

use crate::app::PlacementState;
use pandemonium_sim_api::{Command, CommandKind, TilePos};
use winit::keyboard::Key;
use winit::window::Window;

use super::orders;
use crate::app::App;
use crate::ui::ButtonAction;
use crate::HUMAN;
use pandemonium_engine::audio::{AudioCue, AudioSink};
use pandemonium_engine::MatchHost;
use pandemonium_sim_api::{EntityId, Vec2Fx};

impl App {
    // --- the event-translation methods (verbatim from app/mod.rs) ---
    /// Converts window pixel coordinates to NDC (Y up).
    pub(crate) fn to_ndc(window: &Window, x: f64, y: f64) -> (f32, f32) {
        let size = window.inner_size();
        (
            (x / size.width.max(1) as f64) as f32 * 2.0 - 1.0,
            1.0 - (y / size.height.max(1) as f64) as f32 * 2.0,
        )
    }

    /// The cursor's current NDC position (M8: used by the AttackMove hotkey,
    /// which fires on key-down without a fresh cursor event). Falls back to
    /// the screen center when the cursor hasn't moved yet.
    pub(crate) fn cursor_ndc(&self) -> (f32, f32) {
        let Some(window) = &self.window else {
            return (0.0, 0.0);
        };
        let (x, y) = self.drag_current.unwrap_or_else(|| {
            let size = window.inner_size();
            (size.width as f64 / 2.0, size.height as f64 / 2.0)
        });
        Self::to_ndc(window, x, y)
    }

    /// Whether the selection is exactly one producing structure (the
    /// right-click rally case, §11.4) — extracted so the context order and
    /// the cursor's order hint agree on what a producer selection means.
    pub(crate) fn selection_is_single_producer(&self) -> bool {
        self.selection.len() == 1 && {
            let selected = self.selection[0];
            self.latest_view.as_ref().is_some_and(|view| {
                view.production
                    .iter()
                    .any(|queue| queue.producer == selected)
            })
        }
    }

    /// The pick under a single left click: the nearest own entity to the
    /// cursor within the pick radius (`None` on empty ground). M10.2
    /// (PLAN §1.1): what the *click then does* is the release handler's
    /// decision — select, shift-toggle, or double-click expand — this only
    /// answers "what is under the cursor".
    pub(crate) fn pick_own_entity(&self, ndc: (f32, f32)) -> Option<EntityId> {
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
    pub(crate) fn box_pick(&self, a: (f32, f32), b: (f32, f32)) -> Vec<EntityId> {
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
    /// M10.2 Phase 4 (PLAN §4.2, A-120): "acknowledged" means SUBMITTED —
    /// stamped, accepted into the host's queue, pinged on the ground. The
    /// sim's own verdict is a later, separate fact: a rejection surfaces
    /// through the refusal feedback and does not retract the ack cue.
    pub(crate) fn submit_order(&mut self, kind: CommandKind, ping_at: Option<Vec2Fx>) {
        // PLAN-M10.2 §3.3 (A-113): spectate silences the human's order path
        // at the orders layer — a hidden-but-live path would inject P1
        // commands into a match the "player" does not own. Silently: a
        // refusal cue under a spectator's idle clicks is noise.
        if !self.match_mode.human_orders() {
            return;
        }
        // And no order ever leaves a menu (a menu-only session submits
        // nothing — the belt under the input routing).
        let Some(host) = &mut self.host else {
            return;
        };
        self.command_seq += 1;
        self.commands_submitted += 1;
        if let Some(at) = ping_at {
            self.feedback.ping(at, self.frames_presented);
            self.last_order = Some((self.command_seq, at));
        }
        host.submit(Command::new(HUMAN, 0, self.command_seq, kind));
        // The submission's own sound (PLAN §4.2's command acknowledgment).
        self.audio.on_cue(AudioCue::CommandAck);
    }

    /// M9.1 (plan §11.3): the right-click context command — what is under
    /// the cursor decides the order. An enemy issues Attack (the player's
    /// only way to order an attack — previously impossible), a resource
    /// node issues Gather for the selection's workers, open ground issues
    /// Move. The gate stays the authority; refusals surface as feedback.
    pub(crate) fn issue_context_order(&mut self, ndc: (f32, f32)) {
        if self.selection.is_empty() {
            return;
        }
        // A single selected producer sets its rally with a right-click (the
        // §11.4 rally point; buildings cannot move, so the ground context
        // order means "deliver here", not "go there").
        if self.selection_is_single_producer() {
            let selected = self.selection[0];
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
    pub(crate) fn issue_stop(&mut self) {
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
    pub(crate) fn issue_attack_move(&mut self, ndc: (f32, f32)) {
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
    pub(crate) fn assign_control_group(&mut self, index: usize) {
        if index < self.control_groups.len() {
            self.control_groups[index] = self.selection.clone();
        }
    }

    /// Runs a bottom-bar button press (the input pass hit-tested it). Every
    /// action funnels through [`App::submit_order`] — the same gate, the
    /// same refusal cues (FD-2).
    pub(crate) fn press_button(&mut self, action: ButtonAction) {
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
    pub(crate) fn place_pending_structure(&mut self) {
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
    pub(crate) fn recall_control_group(&mut self, index: usize) {
        if index >= self.control_groups.len() {
            return;
        }
        let live: std::collections::BTreeSet<EntityId> = self
            .view_snapshot()
            .entities
            .iter()
            .map(|entity| entity.id)
            .collect();
        let next = self.control_groups[index]
            .iter()
            .copied()
            .filter(|id| live.contains(id))
            .collect();
        self.set_selection(next);
    }

    /// M10.2 Phase 4 (PLAN §4.2, A-122): the one selection gate — every
    /// user-driven selection change funnels through here so the selection
    /// click cue fires exactly when the membership actually changes (a
    /// redundant re-select of the same members is not a change; a clear,
    /// a toggle-off, a recall, and a box-select are). The match reset
    /// bypasses this gate on purpose — a fresh match's empty selection is
    /// not a click.
    pub(crate) fn set_selection(&mut self, next: Vec<EntityId>) {
        if next != self.selection {
            self.selection = next;
            self.audio.on_cue(AudioCue::SelectionClick);
        }
    }
}

// --- key-state helpers (verbatim from main.rs) ---

pub(crate) fn set_key(
    keys: &mut BTreeSet<Key<&'static str>>,
    character: &'static str,
    pressed: bool,
) {
    let key = Key::Character(character);
    if pressed {
        keys.insert(key);
    } else {
        keys.remove(&key);
    }
}
