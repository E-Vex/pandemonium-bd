//! The balance battery (plan §19's expansion test, Update 4: the stat
//! rebalance — B-031): the roster's counterplay laws, measured on the REAL
//! content through the REAL simulation, pinned executably.
//!
//! Every law is one equal-ore engagement on an open field built from the
//! shipping entity definitions (the default Crossroads bundle): both sides
//! march at each other with `AttackMove`, the eleven-stage pipeline fights
//! the fight, and the survivor count decides the verdict. The battery is
//! the rebalance's acceptance suite — a stat edit that flips a law turns
//! this suite red.
//!
//! The harness is deliberately brutal-simple: no micro, no focus-fire
//! scripting, no retreats — the auto-acquisition and the movement/steering
//! layers do the fighting, exactly as they do for a player's attack-move
//! blob. Laws are stated per equal ORE (not per unit-count): the rebalance's
//! economy is ore-for-ore.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pandemonium_content::ContentBundle;
use pandemonium_fx::Fx;
use pandemonium_sim::{Sim, SpawnDef, TrivialWorld};
use pandemonium_sim_api::{
    Command, CommandKind, ControllerKind, EntityId, MatchSetup, PlayerId, PlayerSetup, Vec2Fx,
};

/// A position from milli-tile coordinates.
fn pos(x: i32, y: i32) -> Vec2Fx {
    Vec2Fx::new(Fx::from_milli(x), Fx::from_milli(y))
}

/// The repository's content directory.
fn repo_content() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("content")
}
fn bundle() -> &'static ContentBundle {
    static BUNDLE: OnceLock<ContentBundle> = OnceLock::new();
    BUNDLE.get_or_init(|| ContentBundle::load_dir(&repo_content()).expect("content loads"))
}

/// One side's force: (entity id, count).
type Force = Vec<(&'static str, usize)>;

/// The kind id of an entity by its id string (kinds are positional over the
/// entity files in filename sort — resolve, never hand-count).
fn kind_of(content: &ContentBundle, id: &str) -> u32 {
    content
        .entities
        .iter()
        .position(|entity| entity.id == id)
        .unwrap_or_else(|| panic!("no entity '{id}' in the bundle")) as u32
}

/// The ore cost of an entity by its id string.
fn ore_of(content: &ContentBundle, id: &str) -> i64 {
    content
        .entities
        .iter()
        .find(|entity| entity.id == id)
        .unwrap_or_else(|| panic!("no entity '{id}' in the bundle"))
        .cost_ore
}

/// The total ore of a force.
fn force_ore(content: &ContentBundle, force: &Force) -> i64 {
    force
        .iter()
        .map(|(id, count)| ore_of(content, id) * *count as i64)
        .sum()
}

/// One side's closing state: entity count and the survivors' mean health
/// as a permille of full (the margin the laws are judged on).
struct Closing {
    entities: usize,
    hp_permille: u32,
}

fn closing(sim: &Sim, side: PlayerId) -> Closing {
    let snapshot = sim.snapshot();
    let mine: Vec<&pandemonium_sim_api::EntityView> = snapshot
        .entities
        .iter()
        .filter(|entity| entity.owner == side)
        .collect();
    let entities = mine.len();
    let hp_permille = if entities == 0 {
        0
    } else {
        mine.iter()
            .map(|entity| entity.hp_fraction_milli)
            .sum::<u32>()
            / entities as u32
    };
    Closing {
        entities,
        hp_permille,
    }
}

/// The matchup world: the real bundle's kinds and rules on a fully open
/// 64x64 field (passability is a test-local override — the laws under test
/// are the ROSTER's, not the map's), with both forces as the initial
/// spawns.
fn matchup_world(content: &ContentBundle, p0: &Force, p1: &Force) -> TrivialWorld {
    let mut world = content.world();
    world.passability = TrivialWorld::open_passability(world.width_tiles, world.height_tiles);
    world.buildability = TrivialWorld::open_buildability(world.width_tiles, world.height_tiles);
    world.scheduled_spawns.clear();
    world.initial_spawns.clear();

    // Army blocks: 8 rows deep, 1.6-tile column pitch, vanguards 7 tiles
    // apart at the field's waist — a MEETING ENGAGEMENT: every pairing is
    // inside every kind's vision (the Attack gate refuses fog-hidden
    // targets, FD-8), so the duel starts clean and the laws measure the
    // fight, not the approach march. Player 0 anchors west, player 1 east.
    let block = |force: &Force, owner: PlayerId, x0: i32, dir: i32| {
        let mut spawns = Vec::new();
        let mut column = 0i32;
        let mut row = 0i32;
        for (id, count) in force {
            for _ in 0..*count {
                let x = x0 + dir * column * 1600;
                let y = 26_000 + row * 1600 - 5_600;
                spawns.push(SpawnDef {
                    owner,
                    kind: pandemonium_sim_api::KindId(kind_of(content, id)),
                    pos: pos(x, y),
                });
                row += 1;
                if row >= 8 {
                    row = 0;
                    column += 1;
                }
            }
            // Each new kind starts a fresh column block.
            if row != 0 {
                row = 0;
                column += 1;
            }
        }
        spawns
    };

    world
        .initial_spawns
        .extend(block(p0, PlayerId(0), 28_500, -1));
    world
        .initial_spawns
        .extend(block(p1, PlayerId(1), 35_500, 1));
    world
}

/// Runs one engagement and reports both sides' closing states (the duel
/// core the laws judge).
fn duel(content: &ContentBundle, p0: &Force, p1: &Force, tick_cap: u32) -> (Closing, Closing) {
    duel_seeded(content, p0, p1, tick_cap, 7)
}

fn duel_seeded(
    content: &ContentBundle,
    p0: &Force,
    p1: &Force,
    tick_cap: u32,
    seed: u64,
) -> (Closing, Closing) {
    // `engage` drives to a verdict; re-run with the closings exposed. (One
    // pass, kept simple: the verdict is derivable from the closings.)
    let world = matchup_world(content, p0, p1);
    let setup = MatchSetup {
        seed,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Human,
            },
        ],
    };
    let mut sim = Sim::new(&world, setup);
    let roster = |side: PlayerId| -> Vec<EntityId> {
        sim.snapshot()
            .entities
            .iter()
            .filter(|entity| entity.owner == side)
            .map(|entity| entity.id)
            .collect()
    };
    let p0_units = roster(PlayerId(0));
    let p1_units = roster(PlayerId(1));
    let mut script: Vec<Command> = Vec::new();
    for (side, mine, theirs) in [
        (PlayerId(0), &p0_units, &p1_units),
        (PlayerId(1), &p1_units, &p0_units),
    ] {
        for (seq, unit) in mine.iter().enumerate() {
            script.push(Command {
                tick: 0,
                issuer: side,
                seq: seq as u32,
                queue: false,
                kind: CommandKind::Attack {
                    units: vec![*unit],
                    target: theirs[seq % theirs.len()],
                },
            });
        }
    }
    let mut sorted: Vec<&Command> = script.iter().collect();
    sorted.sort_by_key(|command| command.tick);
    let mut cursor = 0;
    while sim.tick() < tick_cap {
        let tick = sim.tick();
        let mut feed: Vec<Command> = Vec::new();
        while cursor < sorted.len() && sorted[cursor].tick == tick {
            feed.push(sorted[cursor].clone());
            cursor += 1;
        }
        sim.step(&feed);
        if closing(&sim, PlayerId(0)).entities == 0 || closing(&sim, PlayerId(1)).entities == 0 {
            break;
        }
    }
    (closing(&sim, PlayerId(0)), closing(&sim, PlayerId(1)))
}

/// A two-player setup at a chosen seed.
fn two_player_setup(seed: u64) -> MatchSetup {
    MatchSetup {
        seed,
        players: vec![
            PlayerSetup {
                player: PlayerId(0),
                controller: ControllerKind::Human,
            },
            PlayerSetup {
                player: PlayerId(1),
                controller: ControllerKind::Human,
            },
        ],
    }
}

/// The standoff world: like the matchup world, but the two forces sit TEN
/// tiles apart (inside the longest acquire ranges, outside the shortest
/// weapon ranges) — for laws about what out-reaches what, with nobody
/// ordered to move.
fn standoff_world(p0: &Force, p1: &Force) -> TrivialWorld {
    let mut world = matchup_world(bundle(), p0, p1);
    for spawn in &mut world.initial_spawns {
        if spawn.owner == PlayerId(0) {
            spawn.pos = pos(25_000, spawn.pos.y.raw() * 1000 / 65_536);
        } else {
            spawn.pos = pos(35_000, spawn.pos.y.raw() * 1000 / 65_536);
        }
    }
    world
}

// ---------------------------------------------------------------------------
// The battery: the counterplay laws, one test each, seed-swept.
//
// Ore pairings are exact wherever the price lattice allows (8 rifleman =
// 600 = 10 raiders = 3 guardians; 5 lancers = 800 = 4 guardians); the laws
// that cannot sit on the lattice state their band in the doc comment. The
// seeds sweep the harness's only free inputs — a law that is a coin flip
// fails somewhere in the sweep.
//
// The web these laws pin (B-031's measured table):
//
//   guardians  > riflemen    (L2 — the anchor breaks the line)
//   riflemen   > raiders     (L1 — the line holds against speed)
//   raiders    > ballistas   (L5 — speed runs down siege)
//   raiders    > turrets     (L10 — speed swarms static defense)
//   lancers    > guardians   (L3 — the piercer breaks armor)
//   riflemen   > ballistas   (L6 — siege does not duel the line)
//   ballistas  > turrets     (L8 — siege breaks the turtle, standoff)
//   turrets    > riflemen    (L9 — static defense holds the line's press)
//   mixed arms > lancers     (L4 — the answer to the piercer is a MIXED
//                             force, never a mono one)
//
// Every kind has a predator; the two "closed corners" of the B-029-era
// table (guardians beat everything at equal ore; ballistas one-shot the
// line for free) are gone.
// ---------------------------------------------------------------------------

/// The laws' seed sweep.
const LAW_SEEDS: [u64; 5] = [7, 13, 42, 99, 1234];

/// Asserts a law across the seed sweep: `expected` names the side that must
/// hold the field, with at least `min_survivors` standing.
fn law(
    name: &str,
    p0: &[(&'static str, usize)],
    p1: &[(&'static str, usize)],
    expected: &str,
    min_survivors: usize,
) {
    let p0: Force = p0.to_vec();
    let p1: Force = p1.to_vec();
    for seed in LAW_SEEDS {
        let (c0, c1) = duel_seeded(bundle(), &p0, &p1, 5400, seed);
        let verdict = match (c0.entities, c1.entities) {
            (0, 0) => "trade",
            (0, n) if n >= min_survivors => "p1",
            (n, 0) if n >= min_survivors => "p0",
            _ => "weak",
        };
        assert_eq!(
            verdict,
            expected,
            "{name} at seed {seed}: p0 {} @{}\u{2030} vs p1 {} @{}\u{2030} \
             (ore {} vs {})",
            c0.entities,
            c0.hp_permille,
            c1.entities,
            c1.hp_permille,
            force_ore(bundle(), &p0),
            force_ore(bundle(), &p1),
        );
    }
}

/// **L1 — the line holds against speed.** Equal ore of riflemen beats the
/// raider swarm: range discipline wears the fast bodies down before they
/// close. (600 ore vs 600.)
#[test]
fn the_line_holds_against_speed() {
    law(
        "the line holds against speed",
        &[("rifleman", 8)],
        &[("raider", 10)],
        "p0",
        3,
    );
}

/// **L2 — armor breaks the line.** Equal ore of guardians beats the
/// rifleman wall: the 30-damage volley now needs THREE hits per rifleman
/// (B-031's 65-hp line; B-029's 60-hp line died in two), and the range-8
/// anchor fires through the approach. The anchor's predator is the piercer
/// (L3). (600 vs 600.)
#[test]
fn armor_breaks_the_line() {
    law(
        "armor breaks the line",
        &[("rifleman", 8)],
        &[("guardian", 3)],
        "p1",
        1,
    );
}

/// **L3 — the piercer breaks armor.** Equal ore of lancers beats equal ore
/// of guardians: five 100-hp bodies focus two 60-damage volleys into each
/// 220-hp anchor and the trade tips. At B-029's numbers this duel ended
/// four-guardians-standing, zero guardian deaths — the billed anti-armor
/// unit could not touch its own target (the B-031 report's headline
/// finding, and the reason the lancer grew). (800 vs 800.)
#[test]
fn the_piercer_breaks_armor() {
    law(
        "the piercer breaks armor",
        &[("guardian", 4)],
        &[("lancer", 5)],
        "p1",
        1,
    );
}

/// **L4 — mixed arms hold the piercer.** A mixed rifleman-plus-guardian
/// force beats a PURE lancer force of MORE ore: the guardians soak the
/// piercer's volleys while the line's volume melts the platforms. No mono
/// force in the roster holds equal-ore lancers — that is by design: the
/// piercer is the anti-mono weapon, and the answer to it is composition,
/// not a bigger mono ball. (775 vs 800 — the mixed side wins at a 3% ore
/// deficit.)
#[test]
fn mixed_arms_hold_the_piercer() {
    law(
        "mixed arms hold the piercer",
        &[("rifleman", 5), ("guardian", 2)],
        &[("lancer", 5)],
        "p0",
        1,
    );
}

/// **L5 — speed runs down siege.** Raiders (600 ore) run down a slightly
/// cheaper ballista pair (550): the siege platform's 60-damage volley
/// one-shots a raider every four seconds and the swarm does not care. The
/// band is stated: siege spends 550 against speed's 600.
#[test]
fn speed_runs_down_siege() {
    law(
        "speed runs down siege",
        &[("raider", 10)],
        &[("ballista", 2)],
        "p0",
        3,
    );
}

/// **L6 — siege does not duel the line.** Unsupported ballistas lose the
/// open field to equal-ish ore of riflemen: B-031's 65-hp rifleman ends
/// the exact-one-shot era (a kill now costs two 60-damage volleys — one
/// whole volley of overkill), and the survivors close and dismantle the
/// platform. Siege breaks STATIC defense (L8); against anything that
/// moves it dies (L5, this law, and the guardians' charge). (600 vs 550.)
#[test]
fn siege_does_not_duel_the_line() {
    law(
        "siege does not duel the line",
        &[("rifleman", 8)],
        &[("ballista", 2)],
        "p0",
        2,
    );
}

/// **L7 — scouts do not win fights.** Equal ore of pathfinders (twelve
/// bodies, the longest sight on the field) loses the firefight to the
/// raider swarm: vision wins maps, not melees. The last pathfinders
/// outrun their killers (speed 6.0 against 4.0 — the scout's actual job),
/// so this law asserts DOMINANCE, not annihilation: at the tick cap the
/// swarm holds the field with the healthy majority and the scouts are a
/// spent force. (600 vs 600.)
#[test]
fn scouts_do_not_win_fights() {
    for seed in LAW_SEEDS {
        let p0 = vec![("pathfinder", 12)];
        let p1 = vec![("raider", 10)];
        let (c0, c1) = duel_seeded(bundle(), &p0, &p1, 5400, seed);
        assert!(
            c1.entities > c0.entities && c1.entities >= 5 && c0.entities <= 4,
            "the raider swarm must dominate the scouts at seed {seed}: \
             scouts {} @{}\u{2030}, raiders {} @{}\u{2030}",
            c0.entities,
            c0.hp_permille,
            c1.entities,
            c1.hp_permille,
        );
    }
}

/// **L8 — siege breaks the turtle.** Two ballistas dismantle a four-turret
/// battery (550 ore against the battery's 500) from OUTSIDE its reach —
/// ten tiles of standoff, nobody ordered to move, pure acquisition: the
/// turret's 7-tile guns never fire. Pure static defense cannot hold ground
/// against siege investment; the answer to a ballista is a mobile screen
/// (L5, L6), never more turrets.
#[test]
fn siege_breaks_the_turtle() {
    let world = standoff_world(&vec![("turret", 4)], &vec![("ballista", 2)]);
    let mut sim = Sim::new(&world, two_player_setup(7));
    let mut tick = 0;
    while tick < 7200 {
        sim.step(&[]);
        if closing(&sim, PlayerId(0)).entities == 0 {
            break;
        }
        tick += 1;
    }
    let defense = closing(&sim, PlayerId(0));
    let siege = closing(&sim, PlayerId(1));
    assert_eq!(defense.entities, 0, "the battery must be dismantled");
    assert_eq!(
        siege.entities, 2,
        "the ballistas must stand untouched (got {} @{}\u{2030})",
        siege.entities, siege.hp_permille
    );
    assert_eq!(
        siege.hp_permille, 1000,
        "standoff means the turrets never land a shot"
    );
}

/// **L9 — static defense holds the line's press.** One turret (125 ore)
/// beats two riflemen (150): the chasing line funnels — only the front
/// rank ever stands in weapon range — and prepared positions eat a
/// same-cost press of the line. (The press that DOES take turrets is
/// speed: L10, or siege: L8.) (150 vs 125.)
#[test]
fn static_defense_holds_the_press() {
    law(
        "static defense holds the press",
        &[("rifleman", 2)],
        &[("turret", 1)],
        "p1",
        1,
    );
}

/// **L10 — speed swarms the turret.** Six raiders (360 ore) take one
/// turret (125): melee bodies crowd the footprint instead of funneling,
/// and the swarm's combined damage outpaces the turret's one-kill-per-5s
/// cadence. Speed is the anti-turret tool, the line is not. (360 vs 125.)
#[test]
fn speed_swarms_the_turret() {
    law(
        "speed swarms the turret",
        &[("raider", 6)],
        &[("turret", 1)],
        "p0",
        2,
    );
}

/// **The battery is deterministic.** The same law, run twice at the same
/// seed, closes identically (the sim's own A1 pins carry bit-identity;
/// this pins the harness).
#[test]
fn the_battery_is_deterministic() {
    let p0 = vec![("rifleman", 8)];
    let p1 = vec![("guardian", 3)];
    let (a0, a1) = duel(bundle(), &p0, &p1, 5400);
    let (b0, b1) = duel(bundle(), &p0, &p1, 5400);
    assert_eq!((a0.entities, a0.hp_permille), (b0.entities, b0.hp_permille));
    assert_eq!((a1.entities, a1.hp_permille), (b1.entities, b1.hp_permille));
}

// ---------------------------------------------------------------------------
// The Phalanx's laws (B-032, plan §19 Update 5 — the second faction): the
// anvil's counter-web, measured on the real content through the same
// harness, cross-faction (the entity pool is shared, so the battery can
// pit the second faction's kinds against the Legion's directly — the
// §17 cross-faction match's preview).
//
//   hoplites    > riflemen    (P1 — the anvil out-eats the line)
//   guardians   > hoplites    (P2 — the anchor holds the anvil)
//   lancers     > hoplites    (P3 — the piercer breaks the anvil)
//   hoplites    > raiders     (P4 — the anvil eats the swarm)
//   raiders     > onagers     (P6 — the swarm overruns the engines)
//   cataphracts > ballistas   (P7 — shock runs down siege)
//   colossus    > guardians   (P8 — the fortress out-anchors the anchor)
//   lancers     > colossus    (P9 — the piercer drowns the fortress)
//   raiders     > colossus    (P10 — the swarm drowns the fortress, 1.8x ore)
//   onagers     > turrets     (P5 — the engines out-reach the nest, standoff)
//   guardians   > onagers     (P12 — the anchor out-reaches the engines)
//   onagers     > the line    (P11 — the breaker out-breaks the line, a race)
//
// The web is a cycle, not a ladder: the anvil beats the Legion's line and
// swarm, the Legion's anchor and piercer beat the anvil, the fortress
// beats the anchor, and the piercer and the swarm beat the fortress. The
// rifle line does NOT clear a standoff by itself (the measured P6-class
// duel ends 2 onagers standing at 550 vs 600 — a pyrrhic engine win, so
// no law pins it); the Legion's onager answers are the anchor, the
// piercer, and the swarm, and the Phalanx's own wall screens its engines.
// The authored balance is analysis, not playtest (A-230) — the A14
// sessions make the feel calls.
// ---------------------------------------------------------------------------

/// **P1 — the anvil out-eats the line.** Equal ore of hoplites beats the
/// rifleman wall: one hp per ore is the field's best body value, and the
/// 14-damage spear needs five swings per rifleman while the wall closes.
/// (600 vs 600.)
#[test]
fn the_anvil_out_eats_the_line() {
    law(
        "the anvil out-eats the line",
        &[("hoplite", 5)],
        &[("rifleman", 8)],
        "p0",
        2,
    );
}

/// **P2 — the anchor holds the anvil.** Equal ore of guardians beats the
/// hoplite wall: the anchor's 30-damage volley needs four swings per
/// 120-hp hoplite and its range-8 guns fire through the whole approach.
/// The Phalanx's answer to the anchor is the fortress (P8) or the piercer
/// of its own — never the tier-1 wall. (600 vs 600.)
#[test]
fn the_anchor_holds_the_anvil() {
    law(
        "the anchor holds the anvil",
        &[("guardian", 3)],
        &[("hoplite", 5)],
        "p0",
        1,
    );
}

/// **P3 — the piercer breaks the anvil.** The Lancer's 60-damage volley
/// lands exactly twice per 120-hp hoplite — zero waste, the lattice the
/// anvil was authored on — so the piercer cross-punishes the second
/// faction's tier-1 for free: anti-armor is anti-phalanx. (640 vs 600 —
/// the price lattice has no 600 of lancers; the piercer wins with the ore
/// deficit in the other direction at 480 vs 600, measured.)
#[test]
fn the_piercer_breaks_the_anvil() {
    law(
        "the piercer breaks the anvil",
        &[("lancer", 4)],
        &[("hoplite", 5)],
        "p0",
        2,
    );
}

/// **P4 — the anvil eats the swarm.** Equal ore of hoplites beats the
/// raider swarm: the swarm's 6-damage bites need twenty per hoplite, and
/// the wall's 14-damage spears waste nothing on 45-hp bodies (four exact
/// swings). Speed does not run down THIS line — the anvil's price is paid
/// to the anchor and the piercer instead (P2, P3). (600 vs 600.)
#[test]
fn the_anvil_eats_the_swarm() {
    law(
        "the anvil eats the swarm",
        &[("hoplite", 5)],
        &[("raider", 10)],
        "p0",
        2,
    );
}

/// **P6 — the swarm overruns the engines.** Equal ore of raiders deletes
/// the onager line before its slow clock clears a quarter of the swarm:
/// the engines are screened weapons, and a screen left home is a donation.
/// (600 vs 550.)
#[test]
fn the_swarm_overruns_the_engines() {
    law(
        "the swarm overruns the engines",
        &[("raider", 10)],
        &[("onager", 5)],
        "p0",
        3,
    );
}

/// **P7 — shock runs down siege.** Two cataphracts (480) close on two
/// ballistas (550) and delete them: 260 hp eats two volleys with change,
/// and 2.6 tiles/s covers the standoff before the third lands. The
/// Phalanx's siege answer is a body, not a bigger gun — and it is NOT a
/// line answer: the rifle wall out-grinds the shock at equal ore (the
/// cataphract pays for its hooves in cost-efficiency). (480 vs 550 — the
/// shock wins at a 13% ore deficit.)
#[test]
fn shock_runs_down_siege() {
    law(
        "shock runs down siege",
        &[("cataphract", 2)],
        &[("ballista", 2)],
        "p0",
        1,
    );
}

/// **P8 — the fortress out-anchors the anchor.** One colossus (400) beats
/// two guardians (400) at exact parity: 520 hp out-tanks the anchors'
/// combined fire while the 80-damage chunks crack one anchor per three
/// volleys. The anchor's predator in the Legion is the piercer (L3); in
/// the Phalanx it is the fortress — and the fortress's own predators are
/// the piercer (P9) and the swarm (P10). (400 vs 400.)
#[test]
fn the_fortress_out_anchors_the_anchor() {
    law(
        "the fortress out-anchors the anchor",
        &[("colossus", 1)],
        &[("guardian", 2)],
        "p0",
        1,
    );
}

/// **P9 — the piercer drowns the fortress.** Three lancers (480) delete
/// one colossus (400): nine exact volleys into 520 hp while the fortress's
/// slow clock kills one piercer every two swings. The heaviest body on
/// the field dies to the cheapest anti-armor — the web's law, unchanged
/// by size. (480 vs 400.)
#[test]
fn the_piercer_drowns_the_fortress() {
    law(
        "the piercer drowns the fortress",
        &[("lancer", 3)],
        &[("colossus", 1)],
        "p0",
        1,
    );
}

/// **P10 — the swarm drowns the fortress.** Twelve raiders (720) delete
/// one colossus (400) with the majority standing: the 80-damage chunks
/// one-shot a raider every three seconds, and the swarm does not care.
/// The band is deliberate and stated: the fortress is PRICED so its
/// counter needs numbers (1.8x ore) — at equal ore (10 raiders) the duel
/// ends one pristine raider standing, a coin the law refuses to flip on.
/// (720 vs 400.)
#[test]
fn the_swarm_drowns_the_fortress() {
    law(
        "the swarm drowns the fortress",
        &[("raider", 12)],
        &[("colossus", 1)],
        "p0",
        3,
    );
}

/// **P12 — the anchor out-reaches the engines.** Equal ore of guardians
/// beats the onager line without closing: the anchor's 8.0 guns out-range
/// the engines' 7.5, so the stone never lands in range — standoff is a
/// knife edge, and the anchor is on the right side of it. (600 vs 550.)
#[test]
fn the_anchor_out_reaches_the_engines() {
    law(
        "the anchor out-reaches the engines",
        &[("guardian", 3)],
        &[("onager", 5)],
        "p0",
        1,
    );
}

/// **P5 — the engines out-reach the nest.** Five onagers (550) dismantle
/// a four-turret battery (500) from OUTSIDE its reach — the L8 law's
/// shape, run on the Phalanx's poor-man's siege. The margin is the whole
/// design: range 7.5 against the turret's 7.0, half a tile of standoff
/// bought at the price of every other stat (60 hp, a stone every 4 s).
/// Pure static defense cannot hold ground against engine investment; the
/// answer to an onager is a mobile screen (P6, P12), never more turrets.
#[test]
fn the_engines_out_reach_the_nest() {
    // The nest standoff: like the battery's standoff world, but the gap is
    // the onager's own half-tile window — 7.4 tiles, inside the engines'
    // 7.5 reach, outside the turrets' 7.0. Nobody is ordered to move;
    // acquisition does the rest.
    let p0: Force = vec![("onager", 5)];
    let p1: Force = vec![("turret", 4)];
    let world = matchup_world(bundle(), &p0, &p1);
    let mut world = world;
    for spawn in &mut world.initial_spawns {
        if spawn.owner == PlayerId(0) {
            spawn.pos = pos(28_100, spawn.pos.y.raw() * 1000 / 65_536);
        } else {
            spawn.pos = pos(35_500, spawn.pos.y.raw() * 1000 / 65_536);
        }
    }
    let mut sim = Sim::new(&world, two_player_setup(7));
    let mut tick = 0;
    while tick < 7200 {
        sim.step(&[]);
        if closing(&sim, PlayerId(1)).entities == 0 {
            break;
        }
        tick += 1;
    }
    let engines = closing(&sim, PlayerId(0));
    let nest = closing(&sim, PlayerId(1));
    assert_eq!(nest.entities, 0, "the battery must be dismantled");
    assert_eq!(
        engines.entities, 5,
        "the engines must stand untouched (got {} @{}\u{2030})",
        engines.entities, engines.hp_permille
    );
    assert_eq!(
        engines.hp_permille, 1000,
        "standoff means the turrets never land a shot"
    );
}

/// **P11 — the breaker out-breaks the line.** The onager's identity is a
/// RACE, not a duel: at equal ore the engines dismantle a command center
/// in ~484 ticks where the rifle line needs ~795 (and pays 25 more ore
/// for the privilege) — no approach march, no formation jam, stones
/// landing from spawn distance. The measure is per-tick (the harness's
/// one timing law), deterministic at the seed.
#[test]
fn the_breaker_out_breaks_the_line() {
    let race = |force: &[(&'static str, usize)]| -> u32 {
        let cc: Force = vec![("command_center", 1)];
        let world = matchup_world(bundle(), &force.to_vec(), &cc);
        let mut sim = Sim::new(&world, two_player_setup(7));
        let target = sim
            .snapshot()
            .entities
            .iter()
            .find(|entity| entity.owner == PlayerId(1))
            .expect("the CC stands")
            .id;
        let mine: Vec<EntityId> = sim
            .snapshot()
            .entities
            .iter()
            .filter(|entity| entity.owner == PlayerId(0))
            .map(|entity| entity.id)
            .collect();
        let script: Vec<Command> = mine
            .iter()
            .enumerate()
            .map(|(seq, unit)| Command {
                tick: 0,
                issuer: PlayerId(0),
                seq: seq as u32,
                queue: false,
                kind: CommandKind::Attack {
                    units: vec![*unit],
                    target,
                },
            })
            .collect();
        let mut tick = 0u32;
        let mut fed = false;
        while tick < 5400 {
            let feed = if fed { Vec::new() } else { script.clone() };
            fed = true;
            sim.step(&feed);
            if closing(&sim, PlayerId(1)).entities == 0 {
                break;
            }
            tick += 1;
        }
        assert_eq!(
            closing(&sim, PlayerId(1)).entities,
            0,
            "the command center must fall inside the cap"
        );
        tick
    };
    let engines = race(&[("onager", 5)]);
    let line = race(&[("rifleman", 7)]);
    assert!(
        engines * 4 < line * 3,
        "the engines must out-break the line decisively: onagers {engines} \
         ticks vs rifles {line} ticks (550 ore vs 525)"
    );
}
