//! Stage 9 — Vision & fog (plan §9.5, M6): per-player per-tile three-state
//! visibility, maintained incrementally as vision-carrying entities move,
//! spawn, and die.
//!
//! ## States (plan §9.5)
//!
//! Each tile, per player, is in one of three states:
//!
//! - `Hidden`   — never seen; the tile has never been inside any friendly
//!   vision radius.
//! - `Explored` — seen before but not currently visible; the tile was inside
//!   a vision radius at some past tick but isn't right now.
//! - `Visible`  — currently inside a friendly vision radius.
//!
//! ## Where the state lives (A-059)
//!
//! `FogState` is **derived** state, not canonical hashed state. The state
//! hash already includes every entity's position + the Vision capability
//! radius, both of which fully determine the fog state at any tick. Adding
//! the fog bitsets to the hash would be redundant (the same inputs would
//! always produce the same bitsets) and would couple the hash to a per-
//! player derivative that A10 explicitly wants "hashes equal fog on/off".
//! Fog is therefore a derived view: `Sim` carries the `FogState` as a
//! non-hashed cache, recomputed each tick from positions + vision radii.
//!
//! A10's "fog on/off, hashes equal" is then trivially true: the hash never
//! included the fog bitsets, so toggling the observer mode (which only
//! affects which entities `player_view` returns, not the canonical state)
//! cannot change the hash. The targeted test (`a10_fog_integrity_hashes_equal`)
//! pins this.
//!
//! ## Incremental update (plan §9.5)
//!
//! A full recompute at match start sets every tile to `Hidden` and then
//! marks `Visible` every tile inside any friendly vision radius (with the
//! previously-`Visible` tiles becoming `Explored`). Each subsequent tick
//! only needs to update tiles that *changed* vision status — which is at
//! most the symmetric difference of the previous and current vision radii.
//! The Alpha implements a simpler scheme: clear `Visible` for the player,
//! re-mark `Visible` for every tile in any friendly vision radius, and
//! promote any previously-`Visible` tile that's no longer in a radius to
//! `Explored`. This is O(N * V) where N is the player count and V is the
//! area covered by vision radii — cheaper than the per-tile incremental
//! approach at Alpha entity counts, and exactly the same observable
//! result. A truly incremental scheme (only recompute tiles whose vision
//! status changed) is DEBT-004's full repayment and waits for the visible-
//! profile cost call.

use pandemonium_sim_api::{EntityId, FogPreset, PlayerId};

use crate::world::World;

/// One tile's visibility state for one player (plan §9.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TileVisibility {
    /// Never seen.
    #[default]
    Hidden,
    /// Seen before, not currently visible.
    Explored,
    /// Currently inside a friendly vision radius.
    Visible,
}

/// Per-player fog state: a `Vec<Vec<TileVisibility>>` indexed `[player_index][tile_index]`,
/// where `tile_index = y * width + x`. Not part of the canonical state hash
/// (A-059 — fog is derived from positions + Vision radii, which ARE hashed).
///
/// The `reveal_all` flag (B-015) is the `Visible` fog preset's persistent
/// reveal: when set, the fog filter reports every in-map tile `Visible` for
/// every player for the entire match, however the per-tick vision pass
/// churns the rows underneath. It is fog-filter input only — read by
/// [`FogState::tile`] and the `player_view` mapping, never by any system
/// that touches world state (FD-8: fog filters, never alters, simulation
/// state) — and like the rest of `FogState` it is derived, non-hashed
/// state, so the "hashes equal fog on/off" law (A10) still holds.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct FogState {
    /// One entry per player, indexed by the player's position in `world.players`.
    pub players: Vec<Vec<TileVisibility>>,
    /// Map width in tiles (cached for index math).
    width: u32,
    /// Map height in tiles.
    height: u32,
    /// The `Visible` preset's persistent reveal (B-015): every in-map tile
    /// reads as `Visible` for every player, for the entire match. `false`
    /// keeps today's per-tick demotion behavior, byte-identically.
    reveal_all: bool,
}

impl FogState {
    /// Builds an all-Hidden fog state for a match.
    pub(crate) fn new(player_count: usize, width: u32, height: u32) -> Self {
        let tile_count = (width as u64 * height as u64) as usize;
        let players = (0..player_count)
            .map(|_| vec![TileVisibility::Hidden; tile_count])
            .collect();
        Self {
            players,
            width,
            height,
            reveal_all: false,
        }
    }

    /// Builds the fog state a mission's fog preset selects (B-015). `Default`
    /// is exactly [`FogState::new`] — the historical all-Hidden
    /// initialization, byte-identical. `Explored` starts every tile
    /// `Explored` (the per-tick vision pass still marks what units actually
    /// see `Visible` on top). `Visible` starts every tile `Visible` and sets
    /// the persistent reveal flag, so the filter reports the whole map
    /// visible for the entire match — not just tick 0 — because
    /// `advance_vision` would otherwise demote unrefreshed tiles back to
    /// `Explored` (the flag is read by the filter, never by touching sim
    /// state; FD-8). The same init applies to every player.
    pub(crate) fn with_preset(
        player_count: usize,
        width: u32,
        height: u32,
        preset: FogPreset,
    ) -> Self {
        let mut fog = Self::new(player_count, width, height);
        match preset {
            FogPreset::Default => fog,
            FogPreset::Explored => {
                for state in &mut fog.players {
                    state.fill(TileVisibility::Explored);
                }
                fog
            }
            FogPreset::Visible => {
                for state in &mut fog.players {
                    state.fill(TileVisibility::Visible);
                }
                fog.reveal_all = true;
                fog
            }
        }
    }

    /// Whether the persistent reveal (the `Visible` preset) is active. The
    /// `player_view` mapping consults this so the outward fog row is all
    /// `Visible` without rewriting the per-tick rows underneath.
    pub(crate) fn revealed(&self) -> bool {
        self.reveal_all
    }

    /// The visibility of one tile for one player. With the persistent reveal
    /// set (the `Visible` preset) every in-map tile is `Visible`, whatever
    /// the per-tick pass last wrote.
    pub(crate) fn tile(&self, player_index: usize, x: i32, y: i32) -> TileVisibility {
        let Some(state) = self.players.get(player_index) else {
            return TileVisibility::Hidden;
        };
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return TileVisibility::Hidden;
        }
        if self.reveal_all {
            return TileVisibility::Visible;
        }
        state[(y as u32 * self.width + x as u32) as usize]
    }

    /// Mark a tile as `Visible` for one player (called by `advance_vision`).
    fn mark_visible(&mut self, player_index: usize, x: i32, y: i32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        if let Some(state) = self.players.get_mut(player_index) {
            let idx = (y as u32 * self.width + x as u32) as usize;
            state[idx] = TileVisibility::Visible;
        }
    }

    /// End-of-tick pass: every tile that was `Visible` last tick and is no
    /// longer in a vision radius this tick becomes `Explored`. (Called after
    /// the new `Visible` set is computed; tiles already marked `Visible`
    /// this tick are kept, tiles still `Visible` from last tick but not
    /// refreshed become `Explored`.)
    fn promote_visible_to_explored(&mut self, player_index: usize, was_visible: &[bool]) {
        if let Some(state) = self.players.get_mut(player_index) {
            for (idx, was) in was_visible.iter().enumerate() {
                if *was && state[idx] != TileVisibility::Visible {
                    state[idx] = TileVisibility::Explored;
                }
            }
        }
    }
}

/// Runs the per-tick vision update (plan §6.3 stage 9, §9.5).
///
/// For each player, walk every vision-carrying entity they own, mark every
/// tile inside its radius `Visible`, and demote last-tick's `Visible` tiles
/// that didn't get refreshed to `Explored`. Own entities are always visible
/// to their owner (their tile is always at least `Visible`), so the player's
/// own units never disappear into fog under themselves.
///
/// This is the M1 `visible_to` (full-radius scan) hoisted into a per-tick
/// cache; the on-demand `visible_to` becomes a tile lookup against the
/// `FogState`. DEBT-004's full repayment (truly incremental updates) waits
/// for a profile-driven need.
pub(crate) fn advance_vision(fog: &mut FogState, world: &World) {
    // Snapshot which tiles were Visible before this update, so we can demote
    // the unrefreshed ones to Explored, and clear them to Hidden in the SAME
    // pass (B-006: the snapshot and the clear read and write the same bytes
    // in the same ascending index order with no cross-tile or cross-player
    // dependency, so fusing them writes exactly the values the two passes
    // wrote — one map sweep instead of two per player per tick).
    let was_visible: Vec<Vec<bool>> = fog
        .players
        .iter_mut()
        .map(|state| {
            state
                .iter_mut()
                .map(|tile| {
                    let was = matches!(tile, TileVisibility::Visible);
                    if was {
                        *tile = TileVisibility::Hidden; // temporarily — promote_visible_to_explored fixes it
                    }
                    was
                })
                .collect()
        })
        .collect();

    // Walk vision-carrying entities in ascending id order (the store's
    // invariant) and mark every tile in their radius Visible for their owner.
    for (vision_id, def) in &world.vision {
        let Some(entity) = world.entity(*vision_id) else {
            continue;
        };
        // Only player slots contribute to fog (the neutral sentinel's vision
        // reveals nothing — ore nodes don't carry Vision in the Alpha).
        if entity.owner == PlayerId::NEUTRAL {
            continue;
        }
        let Some(player_index) = player_index_of(world, entity.owner) else {
            continue;
        };
        let radius_raw = def.radius.raw().max(0) as u32;
        // Mark every tile whose center is within the radius. Squared-distance
        // compare against (radius_raw)^2 in raw Q16.16 units. B-006: the
        // per-tile arithmetic is written as the integer math those `Fx`
        // constructions evaluated — a tile center's raw coordinate is exactly
        // `tile * 65_536 + 32_768` (`from_int(tile) + from_milli(500)`, both
        // exact, and every value here is far inside the saturating ranges) —
        // so each comparison sees the same integers as before, without
        // constructing two `Fx` values per tile (~340 K tiles per tick at
        // scale 10x). The row bound hoists out of the inner loop (the skipped
        // rows are exactly the tiles the per-tile guard rejected).
        let radius_sq = (radius_raw as u64) * (radius_raw as u64);
        let center_tile_x = entity.pos.x.floor_int();
        let center_tile_y = entity.pos.y.floor_int();
        let pos_x_raw = entity.pos.x.raw() as i64;
        let pos_y_raw = entity.pos.y.raw() as i64;
        // The radius in tiles is ceil(radius_raw / 65536); we scan a square
        // one tile larger on each side to catch the corners.
        let radius_tiles = (radius_raw.div_ceil(65536)) as i32 + 1;
        let width = fog.width as i32;
        let height = fog.height as i32;
        let row_stride = fog.width as usize;
        let state = &mut fog.players[player_index];
        for dy in -radius_tiles..=radius_tiles {
            let ty = center_tile_y + dy;
            if ty < 0 || ty >= height {
                continue;
            }
            let center_y_raw = ty as i64 * 65_536 + 32_768;
            let row = (ty as usize) * row_stride;
            for dx in -radius_tiles..=radius_tiles {
                let tx = center_tile_x + dx;
                if tx < 0 || tx >= width {
                    continue;
                }
                let delta_x = pos_x_raw - (tx as i64 * 65_536 + 32_768);
                let delta_y = pos_y_raw - center_y_raw;
                let dist_sq = (delta_x * delta_x + delta_y * delta_y) as u64;
                if dist_sq <= radius_sq {
                    state[row + tx as usize] = TileVisibility::Visible;
                }
            }
        }
        // The tile the entity itself stands on is always Visible (own unit).
        fog.mark_visible(player_index, center_tile_x, center_tile_y);
    }

    // Demote last-tick's Visible that didn't get refreshed → Explored.
    for (p, was_vis) in was_visible.iter().enumerate() {
        fog.promote_visible_to_explored(p, was_vis);
    }
}

/// The player's index in `world.players` (the slot order is ascending by id).
fn player_index_of(world: &World, player: PlayerId) -> Option<usize> {
    world.players.iter().position(|p| p.player == player)
}

/// On-demand visibility check (replaces the M1 full-radius scan in
/// `command.rs::visible_to`). Returns true when the target is owned by the
/// viewer, or stands on a tile currently marked `Visible` for the viewer.
pub(crate) fn visible_to(
    fog: &FogState,
    world: &World,
    viewer: PlayerId,
    target: EntityId,
) -> bool {
    let Some(target_entity) = world.entity(target) else {
        return false;
    };
    if target_entity.owner == viewer {
        return true;
    }
    let Some(player_index) = player_index_of(world, viewer) else {
        return false;
    };
    let tx = target_entity.pos.x.floor_int();
    let ty = target_entity.pos.y.floor_int();
    matches!(fog.tile(player_index, tx, ty), TileVisibility::Visible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::{CapabilityData, HealthDef, VisionDef};
    use pandemonium_fx::{Fx, Vec2Fx};
    use pandemonium_sim_api::{EntityId, PlayerId};

    /// A 4x4 world with one player-0 vision-carrier at the center of tile
    /// (1,1) — pos (1.5, 1.5) tile — with radius 1500 milli (1.5 tiles).
    /// Tiles within 1.5 tiles of (1.5, 1.5) — tile (1,1) center at dist 0,
    /// (0,0), (2,0), (0,2), (2,2) at dist sqrt(0.5) ≈ 0.707 tile, (1,0),
    /// (0,1), (2,1), (1,2) at dist 0.5 tile — should all be Visible.
    /// (3,3) at dist sqrt(4.5) ≈ 2.12 tile should stay Hidden.
    fn fog_world() -> (World, FogState) {
        let mut world = World::new();
        world.players.push(crate::world::PlayerState::from_setup(
            &pandemonium_sim_api::PlayerSetup {
                player: PlayerId(0),
                controller: pandemonium_sim_api::ControllerKind::Human,
            },
            &[],
        ));
        // Place the entity at the center of tile (1,1) — the tile-center
        // convention the vision marker uses (entity at a tile corner would
        // be 0.5 tile off from any tile's center, which changes the distance
        // math). Real matches spawn at footprint centers, which IS the tile
        // center for 1x1 footprints.
        let center = Vec2Fx::new(
            pandemonium_fx::Fx::from_int(1) + pandemonium_fx::Fx::from_milli(500),
            pandemonium_fx::Fx::from_int(1) + pandemonium_fx::Fx::from_milli(500),
        );
        world.spawn(
            EntityId(1),
            PlayerId(0),
            pandemonium_sim_api::KindId(0),
            center,
            vec![
                CapabilityData::Health(HealthDef {
                    max_hp: 10,
                    hp: 10,
                    regen_per_tick: 0,
                }),
                CapabilityData::Vision(VisionDef {
                    radius: Fx::from_milli(1500),
                }),
            ],
        );
        let fog = FogState::new(1, 4, 4);
        (world, fog)
    }

    #[test]
    fn advance_vision_marks_visible_tiles_for_player_0() {
        let (world, mut fog) = fog_world();
        advance_vision(&mut fog, &world);
        // Tile (1,1) — the entity's own tile — is Visible.
        assert_eq!(fog.tile(0, 1, 1), TileVisibility::Visible);
        // Tile (0,0) — 0.707 tile from (1,1), within 1.5 tile radius.
        assert_eq!(fog.tile(0, 0, 0), TileVisibility::Visible);
        // Tile (2,2) — same.
        assert_eq!(fog.tile(0, 2, 2), TileVisibility::Visible);
        // Tile (3,3) — 2.83 tile from (1,1), outside the radius.
        assert_eq!(fog.tile(0, 3, 3), TileVisibility::Hidden);
    }

    #[test]
    fn visible_tiles_become_explored_when_vision_moves_away() {
        let (mut world, mut fog) = fog_world();
        // Tick 0: vision at (1,1) marks (0,0), (1,1), (2,2) etc. Visible.
        advance_vision(&mut fog, &world);
        assert_eq!(fog.tile(0, 0, 0), TileVisibility::Visible);
        // Move the vision carrier to (3,3) — vision no longer covers (0,0).
        world.entity_mut(EntityId(1)).unwrap().pos = Vec2Fx::from_ints(3, 3);
        advance_vision(&mut fog, &world);
        // (0,0) was Visible last tick, no longer Visible this tick → Explored.
        assert_eq!(fog.tile(0, 0, 0), TileVisibility::Explored);
        // (3,3) is now Visible (the carrier stands there).
        assert_eq!(fog.tile(0, 3, 3), TileVisibility::Visible);
    }

    #[test]
    fn the_explored_preset_starts_every_tile_explored() {
        // B-015: the Explored preset initializes every tile Explored — the
        // same init for every player — before the vision pass lights what
        // the units actually see.
        let fog = FogState::with_preset(2, 4, 4, FogPreset::Explored);
        for player_index in 0..2 {
            for y in 0..4 {
                for x in 0..4 {
                    assert_eq!(
                        fog.tile(player_index, x, y),
                        TileVisibility::Explored,
                        "tile ({x},{y}) of player {player_index}"
                    );
                }
            }
        }
        // The persistent reveal is NOT active: the demotion pass runs as
        // today, so unrefreshed Visible tiles still settle to Explored.
        assert!(!fog.revealed());
    }

    #[test]
    fn the_explored_preset_lets_vision_light_visible_on_top() {
        // The per-tick pass still marks what units see Visible on top of the
        // Explored base.
        let (world, _) = fog_world();
        let mut fog = FogState::with_preset(1, 4, 4, FogPreset::Explored);
        advance_vision(&mut fog, &world);
        assert_eq!(fog.tile(0, 1, 1), TileVisibility::Visible);
        assert_eq!(fog.tile(0, 0, 0), TileVisibility::Visible);
        // Outside the radius the tile keeps its Explored base — not Hidden.
        assert_eq!(fog.tile(0, 3, 3), TileVisibility::Explored);
    }

    #[test]
    fn the_visible_preset_stays_visible_through_the_tick_pass() {
        // B-015: the whole map is visible for the ENTIRE match, not just
        // tick 0 — advance_vision demotes unrefreshed Visible tiles to
        // Explored, so the preset is a persistent reveal flag read by the
        // filter, and moving the carrier away cannot darken anything.
        let (mut world, _) = fog_world();
        let mut fog = FogState::with_preset(1, 4, 4, FogPreset::Visible);
        assert!(fog.revealed(), "the persistent reveal is active");
        // Tick 0: every in-map tile is Visible — the far corners included.
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(fog.tile(0, x, y), TileVisibility::Visible);
            }
        }
        // Many ticks of vision churn with the carrier roaming: the pass runs
        // as today underneath, but the filter's answer never darkens.
        for tick in 0..50 {
            world.entity_mut(EntityId(1)).unwrap().pos =
                Vec2Fx::from_ints(tick % 4, (tick * 3) % 4);
            advance_vision(&mut fog, &world);
        }
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(
                    fog.tile(0, x, y),
                    TileVisibility::Visible,
                    "tile ({x},{y}) stays Visible after the churn"
                );
            }
        }
    }

    #[test]
    fn the_default_preset_is_the_historical_all_hidden_initialization() {
        // B-015: Default is byte-identical to FogState::new — every tile
        // Hidden, no persistent reveal — so a mission without the field
        // changes nothing observable in the fog.
        let new_fog = FogState::new(2, 8, 6);
        let preset_fog = FogState::with_preset(2, 8, 6, FogPreset::Default);
        assert_eq!(new_fog, preset_fog);
        assert!(!preset_fog.revealed());
        assert_eq!(preset_fog.tile(0, 7, 5), TileVisibility::Hidden);
        assert_eq!(preset_fog.tile(1, 0, 0), TileVisibility::Hidden);
    }

    #[test]
    fn visible_to_returns_true_for_own_entities() {
        let (world, fog) = fog_world();
        // EntityId(1) is owned by PlayerId(0); always visible to its owner.
        assert!(visible_to(&fog, &world, PlayerId(0), EntityId(1)));
    }

    #[test]
    fn visible_to_returns_false_for_unseen_enemy() {
        let (mut world, mut fog) = fog_world();
        // Add an enemy unit at (3,3) — outside player 0's vision.
        world.spawn(
            EntityId(2),
            PlayerId(1),
            pandemonium_sim_api::KindId(1),
            Vec2Fx::from_ints(3, 3),
            vec![CapabilityData::Health(HealthDef {
                max_hp: 10,
                hp: 10,
                regen_per_tick: 0,
            })],
        );
        advance_vision(&mut fog, &world);
        assert!(!visible_to(&fog, &world, PlayerId(0), EntityId(2)));
    }

    #[test]
    fn visible_to_returns_true_for_enemy_inside_vision() {
        let (mut world, mut fog) = fog_world();
        // Add an enemy unit at (2,2) — 0.707 tile from (1,1), within 1.5 tile.
        world.spawn(
            EntityId(2),
            PlayerId(1),
            pandemonium_sim_api::KindId(1),
            Vec2Fx::from_ints(2, 2),
            vec![CapabilityData::Health(HealthDef {
                max_hp: 10,
                hp: 10,
                regen_per_tick: 0,
            })],
        );
        advance_vision(&mut fog, &world);
        assert!(visible_to(&fog, &world, PlayerId(0), EntityId(2)));
    }
}
