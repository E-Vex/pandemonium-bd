//! AI hosting on the tick boundary (plan §9.6, M7): the engine owns stepping
//! the simulation (A-003), so it also owns feeding the controllers.
//!
//! [`AiMatchHost`] is the headless shape of that ownership: one match, one
//! controller per driven slot, one tick per [`AiMatchHost::advance`]. Each
//! advance is exactly the plan's controller contract — perceive (a
//! fog-filtered [`PlayerView`] of the tick about to be applied), decide
//! (inside the controller), act (commands appended and fed, through the same
//! validation gate a player's commands pass). Slots without a controller are
//! passive, exactly like a player who issues nothing.
//!
//! The host records every command it fed — rejections included — so a match
//! run under controllers is a plain command log afterwards: re-simulating it
//! with [`pandemonium_sim::run_command_log`] reproduces every checkpoint
//! without running any controller again (FD-1: a match is seed + content +
//! the ordered command log; the controllers are a way to *produce* a log, not
//! part of its identity).
//!
//! [`alpha_plan`] derives the scripted opponent's configuration from the
//! loaded content: kind ids by capability shape (never name-matched here, and
//! the controllers never match anything at all), costs and build times from
//! the entity definitions, start positions from the map, and candidate build
//! ground from a deterministic ring scan around the player's start.

use pandemonium_ai::{AiPlan, Controller, ScriptedController};
use pandemonium_content::ContentBundle;
use pandemonium_content::{CapabilityDef, EntityDef, StartDef};
use pandemonium_sim::CapTemplate;
use pandemonium_sim::{Sim, StepOutput, TrivialWorld};
use pandemonium_sim_api::{
    Command, KindId, MatchSetup, PlayerId, PlayerView, ResourceId, Snapshot, Tick, TilePos, Vec2Fx,
};

/// How many candidate placements each structure kind collects.
const SPOT_LIMIT: usize = 8;

/// How far from the start anchor the placement scan reaches (Chebyshev rings).
const SPOT_SCAN_RINGS: i32 = 12;

/// Where the scan starts: clear of the command center's own ground.
const SPOT_SCAN_INNER: i32 = 3;

/// One running match with controllers: the simulation (owned privately, as in
/// [`crate::MatchHost`]), the controllers, and the growing command log.
pub struct AiMatchHost {
    sim: Sim,
    controllers: Vec<(PlayerId, Box<dyn Controller>)>,
    log: Vec<Command>,
}

impl AiMatchHost {
    /// Starts a controller-driven match. Every controller must drive a slot of
    /// the setup, no slot twice; controllers are invoked in ascending slot
    /// order every tick (part of the match's determinism contract).
    pub fn new(
        world: &TrivialWorld,
        setup: MatchSetup,
        controllers: Vec<(PlayerId, Box<dyn Controller>)>,
    ) -> Self {
        let mut controllers = controllers;
        for (player, _) in &controllers {
            assert!(
                setup.players.iter().any(|entry| entry.player == *player),
                "controller for slot {player:?}, which is not in the match"
            );
        }
        controllers.sort_by_key(|(player, _)| *player);
        assert!(
            controllers.windows(2).all(|pair| pair[0].0 < pair[1].0),
            "one controller per slot at most"
        );
        Self {
            sim: Sim::new(world, setup),
            controllers,
            log: Vec::new(),
        }
    }

    /// One tick: every controller thinks on its fog-filtered view of the tick
    /// being applied, all commands feed the one step, and the log grows by
    /// exactly what was fed.
    pub fn advance(&mut self) -> StepOutput {
        let tick = self.sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        let Self {
            sim,
            controllers,
            log,
        } = self;
        for (player, controller) in controllers.iter_mut() {
            let view = sim.player_view(*player);
            let mut out = Vec::new();
            controller.think(&view, tick, &mut out);
            debug_assert!(
                out.iter()
                    .all(|command| command.tick == tick && command.issuer == *player),
                "controllers must emit commands for their own slot at the view's tick"
            );
            feed.extend(out);
        }
        log.extend(feed.iter().cloned());
        sim.step(&feed)
    }

    /// The current tick (the tick the next advance applies).
    pub fn tick(&self) -> Tick {
        self.sim.tick()
    }

    /// Every command fed to the simulation so far, in feed order — the match's
    /// command log (rejections included; they changed no state).
    pub fn log(&self) -> &[Command] {
        &self.log
    }

    /// The read-only presentation snapshot.
    pub fn snapshot(&self) -> Snapshot {
        self.sim.snapshot()
    }

    /// The fog-filtered view for one player (tests and overlays).
    pub fn player_view(&self, player: PlayerId) -> PlayerView {
        self.sim.player_view(player)
    }

    /// The on-demand canonical state hash.
    pub fn state_hash(&self) -> u64 {
        self.sim.state_hash()
    }

    /// The id allocator watermark.
    pub fn next_entity_id(&self) -> u64 {
        self.sim.next_entity_id()
    }
}

/// Builds the scripted Alpha opponent for one player from the loaded content
/// (plan §9.6's "scripted controller"): the plan derives from the bundle, the
/// controller's RNG seed from the match seed (plan §9.6: seeded, never its own
/// entropy).
pub fn alpha_controller(
    bundle: &ContentBundle,
    world: &TrivialWorld,
    player: PlayerId,
    seed: u64,
) -> ScriptedController {
    ScriptedController::new(alpha_plan(bundle, world, player, seed))
}

/// The scripted opponent's per-player plan, derived from the content by
/// capability shape (A-061): the production lists decide the roster the way
/// the command card decides it for a player — a producer whose list trains
/// combat kinds is the barracks and its list is the army, the other producer
/// trains workers; supply is a structure that provides population; the node
/// kind carries a resource body. Map knowledge (starts, build ground) comes
/// from the world definition — public knowledge a player reads off the screen,
/// not fog-protected state.
pub fn alpha_plan(
    bundle: &ContentBundle,
    world: &TrivialWorld,
    player: PlayerId,
    seed: u64,
) -> AiPlan {
    let entities = &bundle.entities;
    let kind_of = |id: &str| {
        entities
            .iter()
            .position(|entity| entity.id == id)
            .map(|index| KindId(index as u32))
    };
    let def_of = |kind: KindId| entities.get(kind.0 as usize);
    let attacks = |entity: &EntityDef| {
        entity
            .capabilities
            .iter()
            .any(|cap| matches!(cap, CapabilityDef::Attack { .. }))
    };

    // Roster from the faction's production lists.
    let mut command_center = None;
    let mut worker = None;
    let mut barracks = None;
    let mut army: Vec<KindId> = Vec::new();
    for (producer, producibles) in &bundle.faction().production {
        let Some(producer_kind) = kind_of(producer) else {
            continue;
        };
        let kinds: Vec<KindId> = producibles
            .iter()
            .filter_map(|producible| kind_of(producible))
            .collect();
        let Some(first) = kinds.first() else {
            continue;
        };
        if kinds.iter().any(|kind| def_of(*kind).is_some_and(attacks)) {
            if barracks.is_none() {
                barracks = Some(producer_kind);
                army = kinds;
            }
        } else if command_center.is_none() {
            command_center = Some(producer_kind);
            worker = Some(*first);
        }
    }

    // Supply: the cheapest structure that provides population (the command
    // center itself provides population, so it is excluded by kind).
    let mut depot = None;
    let mut depot_cost_ore = i64::MAX;
    for (index, entity) in entities.iter().enumerate() {
        let kind = KindId(index as u32);
        if Some(kind) == command_center || Some(kind) == barracks {
            continue;
        }
        let provides_population = entity
            .capabilities
            .iter()
            .any(|cap| matches!(cap, CapabilityDef::ProvidesPopulation { .. }));
        if entity.footprint().is_some() && provides_population && entity.cost_ore < depot_cost_ore {
            depot_cost_ore = entity.cost_ore;
            depot = Some(kind);
        }
    }

    // The gatherable node kind: the first kind carrying a resource body.
    let node = entities
        .iter()
        .position(|entity| {
            entity
                .capabilities
                .iter()
                .any(|cap| matches!(cap, CapabilityDef::Resource { .. }))
        })
        .map(|index| KindId(index as u32));

    // Costs in the ledger's own shape (the Alpha prices everything in Ore,
    // exactly as the content seam maps `cost_ore`).
    let ore_resource = bundle
        .rules
        .resources
        .iter()
        .position(|resource| resource.id == "ore")
        .map(|index| ResourceId(index as u32));
    let cost_of = |entity: &EntityDef| -> Vec<(ResourceId, i64)> {
        match (entity.cost_ore > 0, ore_resource) {
            (true, Some(resource)) => vec![(resource, entity.cost_ore)],
            _ => Vec::new(),
        }
    };
    let worker_cost = worker.and_then(def_of).map(cost_of).unwrap_or_default();
    let depot_cost = depot.and_then(def_of).map(cost_of).unwrap_or_default();
    let barracks_cost = barracks.and_then(def_of).map(cost_of).unwrap_or_default();
    let army_cost: Vec<Vec<(ResourceId, i64)>> = army
        .iter()
        .map(|kind| def_of(*kind).map(cost_of).unwrap_or_default())
        .collect();
    let army_build_ticks: Vec<u32> = army
        .iter()
        .map(|kind| def_of(*kind).map(|def| def.build_time_ticks).unwrap_or(0))
        .collect();
    let army_pop: Vec<u32> = army
        .iter()
        .map(|kind| {
            def_of(*kind)
                .map(|def| def.population.max(0) as u32)
                .unwrap_or(1)
        })
        .collect();
    let (worker_build_ticks, worker_pop) = worker
        .and_then(def_of)
        .map(|def| (def.build_time_ticks, def.population.max(0) as u32))
        .unwrap_or((0, 1));

    // Map knowledge: the start positions and candidate build ground.
    let starts = &bundle.map.starts;
    let own_start = starts.iter().find(|start| PlayerId(start.player) == player);
    let enemy_start = starts
        .iter()
        .find(|start| PlayerId(start.player) != player)
        .map(|start| start_center(bundle, start));
    let home = own_start.map(|start| start_center(bundle, start));

    let anchor = own_start.map(|start| TilePos {
        x: start.x,
        y: start.y,
    });
    let spots_for = |kind: Option<KindId>| -> Vec<TilePos> {
        let Some(kind) = kind else {
            return Vec::new();
        };
        let Some(def) = def_of(kind) else {
            return Vec::new();
        };
        let Some((w, h)) = def.footprint() else {
            return Vec::new();
        };
        let Some(anchor) = anchor else {
            return Vec::new();
        };
        build_spots(world, anchor, (w, h), SPOT_LIMIT)
    };
    let depot_spots = spots_for(depot);
    let barracks_spots = spots_for(barracks);

    AiPlan {
        player,
        rng_seed: controller_seed(seed, player),
        worker: worker.unwrap_or(KindId(0)),
        command_center: command_center.unwrap_or(KindId(0)),
        depot,
        barracks,
        node,
        army,
        worker_cost,
        depot_cost,
        barracks_cost,
        army_cost,
        worker_build_ticks,
        army_build_ticks,
        army_pop,
        worker_pop,
        home: home.unwrap_or_default(),
        enemy_start,
        depot_spots,
        barracks_spots,
    }
}

/// The controller's RNG seed: the match seed mixed with the slot, so two
/// controllers in one match draw independent streams from the same seed
/// (plan §9.6: the RNG is passed in; the controller never owns entropy).
fn controller_seed(seed: u64, player: PlayerId) -> u64 {
    seed.wrapping_add((u64::from(player.0) + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// A start's base position: the center of the first structure in the faction's
/// starting forces (the command center in the Alpha; the anchor tile center
/// as a fallback for structure-less starts).
fn start_center(bundle: &ContentBundle, start: &StartDef) -> Vec2Fx {
    for force in &bundle.faction().starting_forces {
        if let Some(def) = bundle.entity(&force.entity) {
            if let Some((w, h)) = def.footprint() {
                return Vec2Fx::from_ints(
                    start.x + force.offset.0 + (w as i32) / 2,
                    start.y + force.offset.1 + (h as i32) / 2,
                );
            }
        }
    }
    Vec2Fx::from_ints(start.x + 1, start.y + 1)
}

/// Collects candidate placements for a structure footprint around a start
/// anchor: a deterministic ring-by-ring scan (Chebyshev rings, row-major
/// within each ring, nearest first), keeping tiles whose whole footprint is
/// inside the map, on buildable ground, clear of every static body standing
/// at match start, and not adjacent to a resource node (the M5 lesson: a
/// node's doorstep is a worker's approach lane — nodes also seal chokepoints).
///
/// This is map knowledge, computed once before the match starts: what a player
/// sees when they look at the minimap and plan a base layout. Whether the
/// ground is *still* free when a `Build` command lands is the gate's job, and
/// the controller's sight-verification handles the difference.
pub fn build_spots(
    world: &TrivialWorld,
    anchor: TilePos,
    footprint: (u32, u32),
    limit: usize,
) -> Vec<TilePos> {
    // Static claims at match start, split into structure claims and node
    // tiles (the margin applies to nodes only).
    let mut claimed: Vec<(i32, i32)> = Vec::new();
    let mut node_tiles: Vec<(i32, i32)> = Vec::new();
    for spawn in &world.initial_spawns {
        let Some(template) = world.kinds.get(spawn.kind.0 as usize) else {
            continue;
        };
        let Some(cap) = template.caps.iter().find_map(|cap| match cap {
            CapTemplate::Footprint { w, h } => Some((*w, *h)),
            _ => None,
        }) else {
            continue;
        };
        let half_w = cap.0 as i32 / 2;
        let half_h = cap.1 as i32 / 2;
        let top_left = (
            spawn.pos.x.floor_int() - half_w,
            spawn.pos.y.floor_int() - half_h,
        );
        let is_node = template
            .caps
            .iter()
            .any(|cap| matches!(cap, CapTemplate::Resource { .. }));
        for dy in 0..cap.1 as i32 {
            for dx in 0..cap.0 as i32 {
                let tile = (top_left.0 + dx, top_left.1 + dy);
                if is_node {
                    node_tiles.push(tile);
                } else {
                    claimed.push(tile);
                }
            }
        }
    }

    let fits = |x: i32, y: i32| -> bool {
        let (w, h) = (footprint.0 as i32, footprint.1 as i32);
        // Bounds.
        if x < 0 || y < 0 || x + w > world.width_tiles as i32 || y + h > world.height_tiles as i32 {
            return false;
        }
        for dy in 0..h {
            for dx in 0..w {
                let (tx, ty) = (x + dx, y + dy);
                // Buildable ground.
                let index = ty as usize * world.width_tiles as usize + tx as usize;
                if world.buildability.get(index).copied().unwrap_or(0) == 0 {
                    return false;
                }
                // Clear of static bodies.
                if claimed.contains(&(tx, ty)) {
                    return false;
                }
                // Off nodes' doorsteps (one-tile margin).
                if node_tiles
                    .iter()
                    .any(|(nx, ny)| (nx - tx).abs() <= 1 && (ny - ty).abs() <= 1)
                {
                    return false;
                }
            }
        }
        true
    };

    let mut spots = Vec::new();
    'scan: for ring in SPOT_SCAN_INNER..=SPOT_SCAN_RINGS {
        for y in (anchor.y - ring)..=(anchor.y + ring) {
            for x in (anchor.x - ring)..=(anchor.x + ring) {
                let on_ring = (x - anchor.x).abs() == ring || (y - anchor.y).abs() == ring;
                if !on_ring {
                    continue;
                }
                if fits(x, y) {
                    spots.push(TilePos { x, y });
                    if spots.len() >= limit {
                        break 'scan;
                    }
                }
            }
        }
    }
    spots
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim::{CapTemplate, KindEconomy, KindTemplate, ResourceDef, SpawnDef};
    use pandemonium_sim_api::{CommandKind, ControllerKind, EntityId, PlayerSetup};
    use std::cell::RefCell;
    use std::rc::Rc;

    fn entity(id: u64) -> pandemonium_sim_api::EntityView {
        pandemonium_sim_api::EntityView {
            id: EntityId(id),
            owner: PlayerId(0),
            kind: KindId(0),
            pos: Vec2Fx::from_ints(0, 0),
            facing: Vec2Fx::ZERO,
            hp_fraction_milli: 1000,
            move_state: pandemonium_sim_api::MoveState::Idle,
        }
    }

    /// A test controller that records the views it saw and optionally emits.
    struct Recorder {
        player: PlayerId,
        seen_views: Rc<RefCell<Vec<PlayerView>>>,
        emit: Box<dyn Fn(Tick) -> Vec<CommandKind>>,
    }

    impl Controller for Recorder {
        fn think(&mut self, view: &PlayerView, tick: Tick, out: &mut Vec<Command>) {
            self.seen_views.borrow_mut().push(view.clone());
            for kind in (self.emit)(tick) {
                out.push(Command::new(self.player, tick, out.len() as u32 + 1, kind));
            }
        }
    }

    fn test_world() -> TrivialWorld {
        TrivialWorld {
            map_id: 0x0048_4F53_5400_0001,
            width_tiles: 32,
            height_tiles: 32,
            passability: TrivialWorld::open_passability(32, 32),
            buildability: TrivialWorld::open_buildability(32, 32),
            kinds: vec![KindTemplate {
                caps: vec![
                    CapTemplate::Health {
                        max_hp: 40,
                        regen_per_tick: 0,
                    },
                    CapTemplate::Move {
                        speed_milli_tiles_per_s: 2600,
                        radius_milli_tiles: 350,
                    },
                    CapTemplate::Vision {
                        radius_milli_tiles: 7000,
                    },
                ],
                economy: KindEconomy::default(),
            }],
            resources: vec![ResourceDef {
                resource: ResourceId(0),
                starting: 200,
            }],
            production: vec![],
            base_population_cap: 0,
            initial_spawns: vec![
                SpawnDef {
                    owner: PlayerId(0),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(8, 8),
                },
                SpawnDef {
                    owner: PlayerId(1),
                    kind: KindId(0),
                    pos: Vec2Fx::from_ints(24, 24),
                },
            ],
            scheduled_spawns: vec![],
            spawn_jitter_milli: 0,
        }
    }

    fn two_player_setup() -> MatchSetup {
        MatchSetup {
            seed: 5,
            players: vec![
                PlayerSetup {
                    player: PlayerId(0),
                    controller: ControllerKind::Ai,
                },
                PlayerSetup {
                    player: PlayerId(1),
                    controller: ControllerKind::Ai,
                },
            ],
        }
    }

    #[test]
    fn controllers_see_fog_filtered_views_and_feed_the_gate() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let views = seen.clone();
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
            PlayerId(0),
            Box::new(Recorder {
                player: PlayerId(0),
                seen_views: views,
                emit: Box::new(|tick| {
                    if tick == 0 {
                        vec![CommandKind::Move {
                            units: vec![EntityId(1)],
                            target: Vec2Fx::from_ints(16, 8),
                        }]
                    } else {
                        Vec::new()
                    }
                }),
            }),
        )];
        let mut host = AiMatchHost::new(&test_world(), two_player_setup(), controllers);

        let first = host.advance();
        // The view the controller saw at tick 0 is exactly the host's view.
        assert_eq!(seen.borrow().len(), 1);
        // Clone the view out: holding the borrow would deadlock the next
        // controller think (RefCell).
        let view = seen.borrow()[0].clone();
        assert_eq!(view.tick, 0);
        assert_eq!(view.player, PlayerId(0));
        // Fog: the far enemy is absent, the own unit is present.
        assert!(view.entities.iter().any(|e| e.id == EntityId(1)));
        assert!(!view.entities.iter().any(|e| e.id == EntityId(2)));
        assert!(
            first
                .events
                .iter()
                .all(|event| !matches!(event, pandemonium_sim_api::Event::CommandRejected { .. })),
            "the move order is valid: {:?}",
            first.events
        );

        // The move took effect and the log recorded the command.
        assert_eq!(host.log().len(), 1);
        for _ in 0..200 {
            host.advance();
        }
        let snapshot = host.snapshot();
        let moved = snapshot
            .entities
            .iter()
            .find(|e| e.id == EntityId(1))
            .expect("alive");
        assert!(
            moved.pos.x > entity(1).pos.x,
            "the ordered unit moved: {:?}",
            moved.pos
        );
    }

    #[test]
    fn the_log_records_rejections_verbatim() {
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
            PlayerId(1),
            Box::new(Recorder {
                player: PlayerId(1),
                seen_views: Rc::new(RefCell::new(Vec::new())),
                emit: Box::new(|_| {
                    // Not the issuer's unit: refused, recorded, no state change.
                    vec![CommandKind::Move {
                        units: vec![EntityId(1)],
                        target: Vec2Fx::from_ints(16, 8),
                    }]
                }),
            }),
        )];
        let mut host = AiMatchHost::new(&test_world(), two_player_setup(), controllers);
        let out = host.advance();
        assert!(out
            .events
            .iter()
            .any(|event| matches!(event, pandemonium_sim_api::Event::CommandRejected { .. })));
        assert_eq!(host.log().len(), 1, "the rejected command is in the log");
        let quiet_hash = host.state_hash();
        for _ in 0..10 {
            host.advance();
        }
        assert_eq!(host.log().len(), 11, "one command per tick, all recorded");
        assert_ne!(quiet_hash, 0);
    }

    #[test]
    fn controllers_run_in_ascending_slot_order() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let mut controllers: Vec<(PlayerId, Box<dyn Controller>)> = Vec::new();
        for player in [PlayerId(1), PlayerId(0)] {
            let order = order.clone();
            controllers.push((
                player,
                Box::new(Recorder {
                    player,
                    seen_views: Rc::new(RefCell::new(Vec::new())),
                    emit: Box::new(move |_| {
                        order.borrow_mut().push(player);
                        Vec::new()
                    }),
                }),
            ));
        }
        let mut host = AiMatchHost::new(&test_world(), two_player_setup(), controllers);
        host.advance();
        host.advance();
        assert_eq!(
            *order.borrow(),
            vec![PlayerId(0), PlayerId(1), PlayerId(0), PlayerId(1)],
            "ascending slot order every tick, regardless of construction order"
        );
    }

    #[test]
    #[should_panic(expected = "not in the match")]
    fn controllers_for_outside_slots_are_refused() {
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
            PlayerId(7),
            Box::new(Recorder {
                player: PlayerId(7),
                seen_views: Rc::new(RefCell::new(Vec::new())),
                emit: Box::new(|_| Vec::new()),
            }),
        )];
        AiMatchHost::new(&test_world(), two_player_setup(), controllers);
    }

    #[test]
    fn the_host_replays_from_its_log_alone() {
        // A controller-driven match and a log-only re-simulation agree on
        // every checkpoint: the controllers produced the log; the log *is*
        // the match (FD-1).
        let controllers: Vec<(PlayerId, Box<dyn Controller>)> = vec![(
            PlayerId(0),
            Box::new(Recorder {
                player: PlayerId(0),
                seen_views: Rc::new(RefCell::new(Vec::new())),
                emit: Box::new(|tick| {
                    if tick.is_multiple_of(30) {
                        vec![CommandKind::Move {
                            units: vec![EntityId(1)],
                            target: Vec2Fx::from_ints(16 + (tick as i32) % 8, 8),
                        }]
                    } else {
                        Vec::new()
                    }
                }),
            }),
        )];
        let setup = two_player_setup();
        let mut host = AiMatchHost::new(&test_world(), setup.clone(), controllers);
        let mut checkpoints = vec![(0, host.state_hash())];
        while host.tick() < 95 {
            let out = host.advance();
            if let Some(hash) = out.hash {
                checkpoints.push((host.tick(), hash));
            }
        }
        let final_hash = host.state_hash();
        if checkpoints.last().map(|cp| cp.0) != Some(host.tick()) {
            checkpoints.push((host.tick(), final_hash));
        }
        let resim = pandemonium_sim::run_command_log(&test_world(), &setup, host.log(), 95);
        assert_eq!(resim.checkpoints, checkpoints);
        assert_eq!(resim.final_hash, final_hash);
    }

    #[test]
    fn build_spots_avoid_static_ground_and_node_doorsteps() {
        // A world with a 2x2 rock patch (unbuildable), a node, and a 2x2
        // structure near the anchor: the scan must dodge all three.
        let mut world = test_world();
        let mut buildability = TrivialWorld::open_buildability(32, 32);
        for tile in [(10, 10), (11, 10), (10, 11), (11, 11)] {
            let index = tile.1 as usize * 32 + tile.0 as usize;
            buildability[index] = 0;
        }
        world.buildability = buildability;
        // A node kind (kind 1: resource + footprint) and a structure (kind 2:
        // footprint) standing near the anchor at (8,8).
        world.kinds.push(KindTemplate {
            caps: vec![
                CapTemplate::Resource {
                    resource: ResourceId(0),
                    amount: 1500,
                },
                CapTemplate::Footprint { w: 2, h: 2 },
            ],
            economy: KindEconomy::default(),
        });
        world.kinds.push(KindTemplate {
            caps: vec![CapTemplate::Footprint { w: 2, h: 2 }],
            economy: KindEconomy::default(),
        });
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId::NEUTRAL,
            kind: KindId(1),
            pos: Vec2Fx::from_ints(6, 6),
        });
        world.initial_spawns.push(SpawnDef {
            owner: PlayerId(0),
            kind: KindId(2),
            pos: Vec2Fx::from_ints(8, 10),
        });

        let spots = build_spots(&world, TilePos { x: 8, y: 8 }, (2, 2), 8);
        assert!(!spots.is_empty(), "open ground exists further out");
        for spot in &spots {
            // No spot on the rock patch, the node, or the structure — and
            // always inside the map.
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (tx, ty) = (spot.x + dx, spot.y + dy);
                assert!(tx >= 0 && ty >= 0 && tx < 32 && ty < 32);
                // The rock patch (10..=11, 10..=11).
                assert!(
                    !((10..=11).contains(&tx) && (10..=11).contains(&ty)),
                    "spot {spot:?} on the rocks"
                );
                // The node's tiles (5..=6, 5..=6) plus the one-tile margin.
                assert!(
                    !((4..=7).contains(&tx) && (4..=7).contains(&ty)),
                    "spot {spot:?} sits on the node's doorstep"
                );
                // The structure's tiles (7..=8, 9..=10).
                assert!(
                    !((7..=8).contains(&tx) && (9..=10).contains(&ty)),
                    "spot {spot:?} overlaps the structure"
                );
            }
        }
    }

    #[test]
    fn build_spots_scan_nearest_first() {
        let world = test_world();
        let spots = build_spots(&world, TilePos { x: 8, y: 8 }, (2, 2), 4);
        // Ring 3 is the first legal ring (the map is open): every spot sits at
        // Chebyshev distance 3, collected row-major.
        assert!(!spots.is_empty());
        for spot in &spots {
            let distance = ((spot.x - 8).abs()).max((spot.y - 8).abs());
            assert_eq!(distance, 3, "first ring first: {spot:?}");
        }
    }
}
