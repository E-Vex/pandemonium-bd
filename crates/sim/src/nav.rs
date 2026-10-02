//! The navigation layer (plan §9.1.1): a tile grid over the world's
//! passability data and A* pathfinding on it.
//!
//! Determinism contract: the search is 8-connected with **no corner cutting**
//! (a diagonal step requires both orthogonal neighbors passable), integer
//! costs in milli-tiles (straight 1000, diagonal 1414 — the floor of the
//! diagonal's true cost, so the octile heuristic stays admissible), and a
//! total-order heap key `(f, h, tile index)` so every pop is unique and the
//! expanded sequence — and therefore the returned path — is a pure function
//! of the grid and the endpoints.
//!
//! Leg safety, two cases: (1) the **straight beeline** — when the exact
//! segment from the mover to the goal touches only passable tiles (checked
//! by an integer supercover walk of that very segment, corner crossings
//! included), the path is just the goal, and the leg is safe by validation;
//! beelines also keep groups converging on one destination on *divergent*
//! lanes instead of funneling through shared tile centers. (2) every other
//! route walks **tile centers**: the start tile's center first (the first
//! leg then stays inside the mover's own tile — a convex square — no matter
//! where in it the mover stands), then adjacent passable tiles (no corner
//! cutting), and the exact goal last (a within-goal-tile leg). Steering
//! additionally refuses any step that would land on blocked terrain, so a
//! collision-displaced mover can never walk into rock either.
//!
//! When the goal tile is blocked or unreachable, the search targets the
//! nearest reachable tile (best heuristic) so units still walk as close as
//! the terrain allows.
//!
//! Everything here is pure integer math (plan §5): no floating-point types,
//! no unordered containers, no wall-clock reads.

use pandemonium_fx::{Fx, Vec2Fx};

/// Cost of one straight tile step, in milli-tiles.
const STRAIGHT_COST: i64 = 1000;
/// Cost of one diagonal tile step, in milli-tiles — the integer floor of the
/// true diagonal cost, which keeps the octile heuristic admissible.
const DIAGONAL_COST: i64 = 1414;

/// The navigation grid: the world's passability in row-major tile order,
/// overlaid with a static-occupancy count (structures and resource nodes
/// block their footprint tiles — plan §9.1.3, M5). Tile `(x, y)` lives at
/// index `y * width + x`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct NavGrid {
    width: u32,
    height: u32,
    passable: Vec<u8>,
    /// Footprint occupancy per tile: how many static bodies claim it. Zero is
    /// free; a tile is traversable only when terrain allows AND no body
    /// claims it. Counts (not booleans) keep spawn/remove pairing honest in
    /// debug builds.
    occupied: Vec<u32>,
}

/// One open-set entry: the total-order key `(f, h, tile)` (plan §9.1.1
/// "deterministic tie-breaks"). `Ord` is reversed so the binary heap's
/// max-order pops the *smallest* key — ties break by smaller `h`, then by
/// smaller tile index.
#[derive(Clone, Copy, PartialEq, Eq)]
struct NodeKey {
    f: i64,
    h: i64,
    tile: u32,
}

impl Ord for NodeKey {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        other
            .f
            .cmp(&self.f)
            .then(other.h.cmp(&self.h))
            .then(other.tile.cmp(&self.tile))
    }
}

impl PartialOrd for NodeKey {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl NavGrid {
    /// Builds the grid from the world's passability bytes. A wrong-length
    /// input is a programmer error caught in debug builds; in release it
    /// degrades deterministically (missing tiles passable, extras dropped).
    pub(crate) fn new(width: u32, height: u32, passability: &[u8]) -> Self {
        let expected = (width as u64 * height as u64) as usize;
        debug_assert_eq!(
            passability.len(),
            expected,
            "passability must be width * height bytes"
        );
        let mut passable = passability.to_vec();
        passable.resize(expected, 1);
        passable.truncate(expected);
        Self {
            width,
            height,
            passable,
            occupied: vec![0; expected],
        }
    }

    /// The grid width in tiles.
    pub(crate) fn width(&self) -> u32 {
        self.width
    }

    /// The grid height in tiles.
    pub(crate) fn height(&self) -> u32 {
        self.height
    }

    /// Whether `(x, y)` is inside the grid and walkable: terrain passable and
    /// no static body claiming the tile.
    pub(crate) fn passable(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return false;
        }
        let index = y as usize * self.width as usize + x as usize;
        self.passable[index] != 0 && self.occupied[index] == 0
    }

    /// Claims a tile for a static body (structures at site spawn, resource
    /// nodes at match start). Out-of-bounds tiles are ignored — placement
    /// validation keeps footprints on the map, so this is defensive only.
    pub(crate) fn occupy(&mut self, x: i32, y: i32) {
        if let Some(index) = self.index(x, y) {
            self.occupied[index] = self.occupied[index].saturating_add(1);
        }
    }

    /// Releases a tile a static body claimed (depletion, death). A release
    /// below zero is a programmer error (unbalanced bookkeeping) — caught in
    /// debug builds, saturating in release.
    pub(crate) fn vacate(&mut self, x: i32, y: i32) {
        if let Some(index) = self.index(x, y) {
            debug_assert!(
                self.occupied[index] > 0,
                "vacating a tile nobody occupies — unbalanced footprint bookkeeping"
            );
            self.occupied[index] = self.occupied[index].saturating_sub(1);
        }
    }

    /// The row-major index of an in-bounds tile.
    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// The tile a position falls into (floor of the coordinates).
    fn tile_of(&self, pos: Vec2Fx) -> (i32, i32) {
        (pos.x.floor_int(), pos.y.floor_int())
    }

    /// The center of tile `(x, y)` in fixed-point tile units.
    fn tile_center(x: i32, y: i32) -> Vec2Fx {
        Vec2Fx::new(
            Fx::from_milli(x * 1000 + 500),
            Fx::from_milli(y * 1000 + 500),
        )
    }

    /// Whether the **exact segment** from `start` to `goal` crosses only
    /// passable tiles: an integer grid traversal (Amanatides–Woo style, all
    /// comparisons by exact cross-multiplication) that enters every tile the
    /// segment passes through, with corner crossings validated like an A*
    /// diagonal (both orthogonal neighbors passable). The start tile is
    /// exempt — the mover stands on it — and the goal tile is checked by the
    /// caller.
    fn segment_clear(&self, start: Vec2Fx, goal: Vec2Fx) -> bool {
        let (x0, y0) = (start.x.raw() as i64, start.y.raw() as i64);
        let (x1, y1) = (goal.x.raw() as i64, goal.y.raw() as i64);
        let (mut tx, mut ty) = (x0.div_euclid(65_536), y0.div_euclid(65_536));
        let (end_tx, end_ty) = (x1.div_euclid(65_536), y1.div_euclid(65_536));
        if (tx, ty) == (end_tx, end_ty) {
            return true; // the whole segment lives in the goal's tile
        }
        let dx = x1 - x0;
        let dy = y1 - y0;
        let step_x = dx.signum();
        let step_y = dy.signum();
        let dx_abs = dx.unsigned_abs() as i128;
        let dy_abs = dy.unsigned_abs() as i128;
        // The next grid line each axis must cross (raw coordinate).
        let mut next_x = if dx == 0 {
            i64::MAX
        } else {
            (tx + i64::from(step_x > 0)) * 65_536
        };
        let mut next_y = if dy == 0 {
            i64::MAX
        } else {
            (ty + i64::from(step_y > 0)) * 65_536
        };
        while (tx, ty) != (end_tx, end_ty) {
            // Which axis crosses first? The crossing parameters are
            // t_x = dist_x / |dx| and t_y = dist_y / |dy| with non-negative
            // step-signed distances, so comparing `dist_x * |dy|` against
            // `dist_y * |dx|` is exact for every sign combination.
            let dist_x = ((next_x - x0) * step_x).max(0) as i128;
            let dist_y = ((next_y - y0) * step_y).max(0) as i128;
            let (enter_x, enter_y, corner);
            if dx == 0 {
                (enter_x, enter_y, corner) = (false, true, false);
            } else if dy == 0 {
                (enter_x, enter_y, corner) = (true, false, false);
            } else if dist_x * dy_abs == dist_y * dx_abs {
                (enter_x, enter_y, corner) = (true, true, true);
            } else if dist_x * dy_abs < dist_y * dx_abs {
                (enter_x, enter_y, corner) = (true, false, false);
            } else {
                (enter_x, enter_y, corner) = (false, true, false);
            }
            // A crossing at or past the goal (t >= 1) ends the walk: the
            // remaining leg stays inside the current tile (or touches its
            // boundary), and the goal tile itself is checked by the caller.
            // Goals that sit exactly on lattice corners land here too —
            // stepping through such a corner would overshoot the goal.
            let done = (enter_x && dist_x >= dx_abs) || (enter_y && dist_y >= dy_abs);
            if done {
                break;
            }
            if corner {
                // The segment passes exactly through a lattice corner: no
                // corner cutting, exactly like an A* diagonal step.
                if !self.passable((tx + step_x) as i32, ty as i32)
                    || !self.passable(tx as i32, (ty + step_y) as i32)
                {
                    return false;
                }
            }
            if enter_x {
                tx += step_x;
                next_x += step_x * 65_536;
            }
            if enter_y {
                ty += step_y;
                next_y += step_y * 65_536;
            }
            if !self.passable(tx as i32, ty as i32) {
                return false;
            }
        }
        true
    }

    /// The octile distance between two tiles, in milli-tiles — the
    /// admissible heuristic for the 8-connected costs.
    fn heuristic(ax: i32, ay: i32, bx: i32, by: i32) -> i64 {
        let dx = (ax - bx).unsigned_abs() as i64;
        let dy = (ay - by).unsigned_abs() as i64;
        let (major, minor) = if dx >= dy { (dx, dy) } else { (dy, dx) };
        // major * 1000 + minor * 414 == major * STRAIGHT + minor * (DIAG - STRAIGHT)
        major * STRAIGHT_COST + minor * (DIAGONAL_COST - STRAIGHT_COST)
    }

    /// Finds a path from `start` to `goal` as fixed-point waypoints,
    /// next-first. `None` means no meaningful progress is possible (the
    /// search could not get closer to the goal than the start already is).
    ///
    /// The returned path's intermediate waypoints are tile centers; the last
    /// waypoint is the exact `goal` when its tile is passable, else the
    /// center of the closest reachable tile.
    pub(crate) fn find_path(&self, start: Vec2Fx, goal: Vec2Fx) -> Option<Vec<Vec2Fx>> {
        let start_tile = self.tile_of(start);
        let goal_tile = self.tile_of(goal);

        // A goal within the mover's own tile: walk straight to the exact point
        // (a within-tile leg is always safe; the goal tile may even be blocked
        // if the mover already stands on it).
        if start_tile == goal_tile {
            return Some(vec![goal]);
        }

        // The straight beeline: when the exact segment start → goal touches
        // only passable tiles, the single waypoint is the goal itself (safe
        // by validation, and divergent lanes for converging groups).
        if self.goal_tile_reachable(goal_tile) && self.segment_clear(start, goal) {
            return Some(vec![goal]);
        }

        // A*: g-scores as flat arrays indexed by tile; the heap key is the
        // total-order (f, h, tile) so pops are unique and deterministic.
        let tiles = self.passable.len();
        let mut g_score = vec![i64::MAX; tiles];
        let mut came_from = vec![u32::MAX; tiles];
        let mut closed = vec![false; tiles];
        let mut open = std::collections::BinaryHeap::new();

        let start_index = self.index_of(start_tile);
        let goal_index = self.index_of(goal_tile);
        let start_h = Self::heuristic(start_tile.0, start_tile.1, goal_tile.0, goal_tile.1);
        g_score[start_index] = 0;
        open.push(NodeKey {
            f: start_h,
            h: start_h,
            tile: start_index as u32,
        });

        // When the goal tile is blocked or unreachable, the best tile seen
        // (by heuristic, then index) becomes the target.
        let mut best_tile = start_index;
        let mut best_key = (start_h, start_index);
        let mut found = start_index == goal_index;

        while let Some(NodeKey { f, h, tile }) = open.pop() {
            let tile = tile as usize;
            if closed[tile] {
                continue; // stale entry — a cheaper path was already expanded
            }
            closed[tile] = true;
            if f - h != g_score[tile] {
                continue; // stale g — the recorded score improved after the push
            }
            if tile == goal_index {
                found = true;
                break;
            }
            let (x, y) = (
                tile as i32 % self.width as i32,
                tile as i32 / self.width as i32,
            );
            for (dx, dy) in NEIGHBOR_OFFSETS {
                let nx = x + dx;
                let ny = y + dy;
                if !self.passable(nx, ny) {
                    continue;
                }
                // No corner cutting: a diagonal step needs both orthogonal
                // neighbors passable too.
                if dx != 0 && dy != 0 && (!self.passable(x + dx, y) || !self.passable(x, y + dy)) {
                    continue;
                }
                let step_cost = if dx != 0 && dy != 0 {
                    DIAGONAL_COST
                } else {
                    STRAIGHT_COST
                };
                let next = ny as usize * self.width as usize + nx as usize;
                if closed[next] {
                    continue;
                }
                let candidate = match g_score[tile].checked_add(step_cost) {
                    Some(value) => value,
                    None => continue,
                };
                if candidate < g_score[next] {
                    g_score[next] = candidate;
                    came_from[next] = tile as u32;
                    let nh = Self::heuristic(nx, ny, goal_tile.0, goal_tile.1);
                    let key = (nh, next);
                    if key < best_key {
                        best_key = key;
                        best_tile = next;
                    }
                    open.push(NodeKey {
                        f: candidate + nh,
                        h: nh,
                        tile: next as u32,
                    });
                }
            }
        }

        let target = if found {
            goal_index
        } else {
            if best_tile == start_index {
                return None; // no progress toward the goal is possible
            }
            best_tile
        };

        // Walk back from the target, then reverse into next-first order.
        let mut tiles_path: Vec<u32> = Vec::new();
        let mut cursor = target;
        while cursor != u32::MAX as usize && cursor != start_index {
            tiles_path.push(cursor as u32);
            cursor = came_from[cursor] as usize;
        }
        tiles_path.reverse();

        // Waypoints, safe by construction: the start tile's center first (the
        // first leg stays inside the mover's own tile), then the path's tile
        // centers, then — when the goal tile itself is passable and was
        // reached — the exact goal (a within-goal-tile leg).
        let mut waypoints: Vec<Vec2Fx> = Vec::with_capacity(tiles_path.len() + 2);
        waypoints.push(Self::tile_center(start_tile.0, start_tile.1));
        for &tile in &tiles_path {
            let x = tile as i32 % self.width as i32;
            let y = tile as i32 / self.width as i32;
            let center = Self::tile_center(x, y);
            if waypoints.last() != Some(&center) {
                waypoints.push(center);
            }
        }
        if found && self.goal_tile_reachable(goal_tile) {
            let goal_center = Self::tile_center(goal_tile.0, goal_tile.1);
            if waypoints.last() != Some(&goal_center) {
                waypoints.push(goal_center);
            }
            waypoints.push(goal);
        }

        if waypoints.len() < 2 {
            return None;
        }
        Some(waypoints)
    }

    /// Whether the goal tile itself can host the exact goal waypoint.
    fn goal_tile_reachable(&self, goal_tile: (i32, i32)) -> bool {
        self.passable(goal_tile.0, goal_tile.1)
    }

    /// The row-major index of a tile (callers keep it in range).
    fn index_of(&self, tile: (i32, i32)) -> usize {
        let x = tile.0.clamp(0, self.width as i32 - 1);
        let y = tile.1.clamp(0, self.height as i32 - 1);
        y as usize * self.width as usize + x as usize
    }
}

/// The eight neighbor offsets in a fixed order — iteration order never
/// changes the result (the heap key is a total order) but stays fixed so the
/// g-score update sequence is stable and reviewable.
const NEIGHBOR_OFFSETS: [(i32, i32); 8] = [
    (1, 0),
    (-1, 0),
    (0, 1),
    (0, -1),
    (1, 1),
    (1, -1),
    (-1, 1),
    (-1, -1),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid from a string sketch: `.` passable, `#` blocked, row-major.
    fn grid_from(sketch: &[&str]) -> NavGrid {
        let height = sketch.len() as u32;
        let width = sketch[0].len() as u32;
        let mut passability = Vec::new();
        for row in sketch {
            for ch in row.chars() {
                passability.push(u8::from(ch != '#'));
            }
        }
        NavGrid::new(width, height, &passability)
    }

    fn at(x: i32, y: i32) -> Vec2Fx {
        Vec2Fx::new(
            Fx::from_milli(x * 1000 + 250),
            Fx::from_milli(y * 1000 + 250),
        )
    }

    #[test]
    fn unobstructed_straight_routes_take_the_beeline() {
        let grid = grid_from(&["......", "......", "......"]);
        // An unobstructed exact segment is a single waypoint: the exact goal.
        let path = grid.find_path(at(1, 1), at(4, 1)).expect("open grid paths");
        assert_eq!(path, vec![at(4, 1)]);
        // Diagonals too — the beeline is validated on the segment itself.
        let diagonal = grid.find_path(at(1, 1), at(4, 2)).expect("open grid paths");
        assert_eq!(diagonal, vec![at(4, 2)]);
    }

    #[test]
    fn beelines_work_in_every_quadrant_direction() {
        // Regression: the crossing comparison once assumed same-sign deltas,
        // so northwest/southeast segments (opposite-sign dx, dy) walked the
        // wrong tiles. Every quadrant's beeline must hold on open ground.
        let grid = grid_from(&[
            ".........",
            ".........",
            ".........",
            ".........",
            ".........",
        ]);
        let center = at(4, 2);
        for (goal, name) in [
            (at(8, 4), "southeast"),
            (at(0, 4), "southwest"),
            (at(8, 0), "northeast"),
            (at(0, 0), "northwest"),
        ] {
            let path = grid.find_path(center, goal).expect("open grid beeline");
            assert_eq!(path, vec![goal], "the {name} beeline is a single waypoint");
        }
    }

    #[test]
    fn beelines_that_cross_rock_fall_back_to_the_search() {
        // The Crossroads regression: the straight segment passes through a
        // wall tile even though a detour exists — the beeline must be
        // refused and the search must route around, on passable tiles only.
        let grid = grid_from(&[
            ".........",
            ".........",
            "#######..",
            ".........",
            ".........",
        ]);
        let path = grid.find_path(at(1, 1), at(7, 3)).expect("the gap detours");
        assert!(path.len() >= 3, "a detour, not the beeline");
        for waypoint in &path {
            let (x, y) = grid.tile_of(*waypoint);
            assert!(grid.passable(x, y), "waypoint on rock at {x},{y}");
        }
        assert_eq!(*path.last().unwrap(), at(7, 3));
    }

    #[test]
    fn a_wall_with_a_gap_is_detoured_not_crossed() {
        let grid = grid_from(&[
            "....#...", "....#...",
            "....#...", // wall at x=4 with a gap at row 3 (0-based row 3)
            "........", "....#...", "....#...",
        ]);
        let start = at(1, 0);
        let goal = at(6, 0);
        let path = grid.find_path(start, goal).expect("the gap is reachable");
        // Every intermediate waypoint sits on a passable tile.
        for waypoint in &path {
            let (x, y) = grid.tile_of(*waypoint);
            assert!(grid.passable(x, y), "waypoint on a blocked tile at {x},{y}");
        }
        // The path is longer than the straight line would be (it detours).
        assert!(path.len() >= 3);
        // The last waypoint is the exact goal (its tile is passable).
        assert_eq!(*path.last().unwrap(), goal);
    }

    #[test]
    fn diagonal_squeezes_never_cut_corners() {
        // The only route to the goal passes the diagonal squeeze between the
        // two blocked tiles — corner cutting is refused, so the path takes
        // the orthogonal way around.
        let grid = grid_from(&["..##", "..#.", "...#", "#..."]);
        let path = grid.find_path(at(0, 0), at(3, 0)).expect("a route exists");
        let mut previous = grid.tile_of(at(0, 0));
        for waypoint in &path {
            let (x, y) = grid.tile_of(*waypoint);
            let (px, py) = previous;
            let (dx, dy) = (x - px, y - py);
            if dx != 0 && dy != 0 {
                assert!(
                    grid.passable(px + dx, py) && grid.passable(px, py + dy),
                    "corner cut into ({x},{y})"
                );
            }
            previous = (x, y);
        }
    }

    #[test]
    fn identical_queries_return_identical_paths() {
        let grid = grid_from(&["....#...", "....#...", "........", "....#...", "....#..."]);
        let first = grid.find_path(at(0, 2), at(6, 2)).expect("reachable");
        for _ in 0..100 {
            assert_eq!(grid.find_path(at(0, 2), at(6, 2)), Some(first.clone()));
        }
    }

    #[test]
    fn sealed_goals_resolve_as_closest_approach_and_sealed_starts_return_none() {
        // A goal sealed off on all sides is not reachable, but the search
        // still finds the closest approachable tile: the path ends adjacent
        // to the pocket instead of failing outright (units walk as close as
        // the terrain allows).
        let grid = grid_from(&[".....", ".###.", ".#.#.", ".###.", "....."]);
        let path = grid
            .find_path(at(0, 0), at(2, 2))
            .expect("closest approach");
        let last = *path.last().unwrap();
        let (lx, ly) = grid.tile_of(last);
        assert!(grid.passable(lx, ly));
        // The pocket's ring is blocked, so the closest approachable tile sits
        // one ring further out (Chebyshev distance 2 to the sealed goal).
        assert!(
            (lx - 2).abs() <= 2 && (ly - 2).abs() <= 2,
            "beside the pocket ring, at {lx},{ly}"
        );
        // A start with every neighbor blocked (a rock sealed by rocks)
        // expands nowhere at all.
        let sealed = grid_from(&[".....", ".###.", ".###.", ".###.", "....."]);
        assert_eq!(
            sealed.find_path(
                Vec2Fx::new(Fx::from_milli(2500), Fx::from_milli(2500)),
                at(4, 0)
            ),
            None
        );
    }

    #[test]
    fn blocked_goals_path_to_the_nearest_reachable_tile() {
        // The goal sits inside a rock cluster; the path must end on the
        // center of a passable tile near it (not the exact blocked goal).
        let grid = grid_from(&["....", ".##.", ".##.", "...."]);
        let goal = at(1, 1); // inside the rocks
        let path = grid.find_path(at(0, 0), goal).expect("a near path exists");
        let last = *path.last().unwrap();
        let (lx, ly) = grid.tile_of(last);
        assert!(
            grid.passable(lx, ly),
            "the final waypoint is on open ground"
        );
        // The final waypoint is a tile center (the exact goal is unreachable).
        assert_eq!(last, NavGrid::tile_center(lx, ly));
        // And it is adjacent to the goal tile.
        let (gx, gy) = grid.tile_of(goal);
        assert!((lx - gx).abs() <= 1 && (ly - gy).abs() <= 1);
    }

    #[test]
    fn occupancy_blocks_tiles_and_release_reopens_them() {
        let mut grid = grid_from(&["....", "....", "....", "...."]);
        assert!(grid.passable(1, 1));
        grid.occupy(1, 1);
        grid.occupy(1, 1); // two bodies may claim the same tile transiently
        assert!(!grid.passable(1, 1), "a claimed tile is not walkable");
        grid.vacate(1, 1);
        assert!(!grid.passable(1, 1), "one claim remains");
        grid.vacate(1, 1);
        assert!(grid.passable(1, 1), "fully released ground reopens");
        // Out-of-bounds claims are defensive no-ops.
        grid.occupy(-1, 0);
        grid.occupy(4, 0);
        grid.vacate(99, 99);
    }

    #[test]
    fn occupied_goals_resolve_to_adjacent_tiles() {
        // A structure sits on the goal tile: the path ends on the center of a
        // free tile next to it, never inside the footprint — the exact shape
        // the gather loop's travel targets rely on.
        let mut grid = grid_from(&["....", "....", "....", "...."]);
        for y in 1..3 {
            for x in 1..3 {
                grid.occupy(x, y);
            }
        }
        let goal = at(1, 1); // inside the 2x2 structure
        let path = grid.find_path(at(0, 0), goal).expect("a near path exists");
        let last = *path.last().unwrap();
        let (lx, ly) = grid.tile_of(last);
        assert!(grid.passable(lx, ly));
        let (gx, gy) = grid.tile_of(goal);
        assert!((lx - gx).abs() <= 1 && (ly - gy).abs() <= 1);
        // No waypoint ever sits on a claimed tile.
        for waypoint in &path {
            let (wx, wy) = grid.tile_of(*waypoint);
            assert!(
                grid.passable(wx, wy),
                "waypoint on claimed tile ({wx},{wy})"
            );
        }
    }

    #[test]
    fn out_of_bounds_tiles_are_blocked() {
        let grid = grid_from(&["...", "...", "..."]);
        assert!(!grid.passable(-1, 0));
        assert!(!grid.passable(0, -1));
        assert!(!grid.passable(3, 0));
        assert!(!grid.passable(0, 3));
        assert!(grid.passable(2, 2));
        // A goal outside the grid degrades to the nearest reachable tile.
        let path = grid
            .find_path(at(1, 1), Vec2Fx::from_ints(9, 1))
            .expect("walks toward the edge");
        let last = *path.last().unwrap();
        assert_eq!(grid.tile_of(last), (2, 1));
    }

    #[test]
    fn intermediate_waypoints_are_tile_centers() {
        let grid = grid_from(&["......", "......", "....#.", "......"]);
        let path = grid.find_path(at(0, 0), at(5, 2)).expect("reachable");
        assert!(path.len() >= 2);
        for waypoint in &path[..path.len() - 1] {
            let (x, y) = grid.tile_of(*waypoint);
            assert_eq!(*waypoint, NavGrid::tile_center(x, y));
        }
    }
}
