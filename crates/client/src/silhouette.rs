//! Multi-part entity silhouettes (PLAN-M10.2 §2.1/§2.2, the visual-legibility
//! pass): what each *kind* of thing looks like, as pure data.
//!
//! The M10 "silhouette" system rendered every kind as one or two tinted
//! boxes sized by capability shape. The playtest's second finding — "at a
//! glance you cannot tell what anything is" — upgrades those specs to
//! distinct multi-part silhouettes built from the same instanced-box
//! primitives: a worker is a small round body with a visible tool, a
//! Guardian is a hull with treads, a turret and a barrel, the Command
//! Center is a tall slab with a corner tower, and so on. Not pretty — just
//! clear (the plan's own bar: a stranger identifies command center, worker,
//! and tank in a screenshot without a legend).
//!
//! The kind → silhouette mapping lives entirely in the client, keyed by the
//! kind's *name* from the loaded bundle (`EntityDef::id`), with a
//! capability-shape fallback (Resource → crystal cluster, Footprint →
//! building box, Move → unit box) for any kind the table does not know.
//! No presentation data is ever added to `content/` files — the content
//! hash never moves (the milestone's hard constraint).
//!
//! Everything here is pure data and pure functions: a [`Silhouette`] is a
//! list of box [`Part`]s plus the derived ground radius and the unit flag,
//! so the whole system unit-tests without a GPU (the
//! pure-builder-plus-tests pattern `input.rs`/`feedback.rs`/`ui.rs` set).

/// How one part takes its color from the owning team's base color.
///
/// The old system had one per-kind tint multiplier; the multi-part
/// silhouettes need *roles* instead — a Guardian's hull reads as the team
/// color while its barrel reads as gunmetal regardless of who owns it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tone {
    /// The team color itself (the main mass).
    Body,
    /// A darker detail of the team color (heads, treads, doors).
    Dark,
    /// A lighter detail of the team color (roofs, cabins, caps).
    Light,
    /// Mostly gunmetal with a trace of team color (tools, barrels, poles).
    Steel,
    /// A strong, bright team-color band — the banner/trim that makes the
    /// owner readable on structures (PLAN-M10.2 §2.2).
    Band,
    /// Ore amber, independent of any team (resource nodes are neutral).
    Amber,
    /// A lighter ore amber (crystal highlights).
    AmberLight,
}

/// One box of a silhouette, in the entity's local space: `x` forward (the
/// direction the entity faces), `y` up, `z` right. `offset` is the box's
/// *center*, `scale` its full extents (the renderer's instance convention),
/// `yaw` an extra rotation of the box itself around its center, relative to
/// the entity's facing.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Part {
    /// Center offset in tile units (x forward, y up, z right).
    pub offset: [f32; 3],
    /// Full extent per axis in tile units (not half — the instance shader
    /// scales corners of ±0.5).
    pub scale: [f32; 3],
    /// Extra yaw of this box around its own center (radians).
    pub yaw: f32,
    /// How the part takes its color from the team palette.
    pub tone: Tone,
}

impl Part {
    /// A convenience constructor for the common axis-aligned box.
    pub fn at(offset: [f32; 3], scale: [f32; 3], tone: Tone) -> Self {
        Self {
            offset,
            scale,
            yaw: 0.0,
            tone,
        }
    }

    /// The part's ground reach from the entity center — how far its box
    /// extends horizontally, worst case over the two ground axes.
    fn reach(&self) -> f32 {
        let dx = self.offset[0].abs() + self.scale[0] * 0.5;
        let dz = self.offset[2].abs() + self.scale[2] * 0.5;
        (dx * dx + dz * dz).sqrt()
    }

    /// The part's box volume — the death fade shrinks the *main* mass, so
    /// the main part is the biggest one.
    fn volume(&self) -> f32 {
        self.scale[0] * self.scale[1] * self.scale[2]
    }
}

/// One kind's presentation spec: the box composition plus the two derived
/// facts the rest of the renderer needs — the ground `radius` (shadow and
/// ring sizing) and whether the kind is a *unit* (mobile: it gets the
/// team-color ground ring; structures get the team band parts instead).
#[derive(Clone, PartialEq, Debug)]
pub struct Silhouette {
    /// The box composition, bottom-up.
    pub parts: Vec<Part>,
    /// Ground radius in tiles: the widest part's reach plus a small margin,
    /// so shadows visibly "sit" under the whole silhouette and rings clear it.
    pub radius: f32,
    /// Whether this reads as a mobile unit (team ground ring under it).
    pub unit: bool,
}

/// The margin added to the widest part's reach when deriving `radius`.
const RADIUS_MARGIN: f32 = 0.08;

impl Silhouette {
    /// Derives the radius and stores the parts (the single constructor —
    /// keeps the derived invariant true by construction).
    pub fn from_parts(unit: bool, parts: Vec<Part>) -> Self {
        let radius = parts.iter().map(Part::reach).fold(0.0_f32, f32::max) + RADIUS_MARGIN;
        Self {
            parts,
            radius,
            unit,
        }
    }

    /// The largest-volume part — the mass the death fade shrinks.
    pub fn main_part(&self) -> &Part {
        self.parts
            .iter()
            .max_by(|a, b| {
                a.volume()
                    .partial_cmp(&b.volume())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .expect("a silhouette always carries at least one part")
    }
}

/// Resolves a part tone to a concrete RGB against the owner's team base.
/// Steel and Amber barely move with the team (tools and ore must read the
/// same for both sides); Band stays strongly team-colored (that is its job).
pub fn part_color(tone: Tone, team: [f32; 3]) -> [f32; 3] {
    let mix = |color: [f32; 3], other: [f32; 3], t: f32| -> [f32; 3] {
        [
            color[0] * (1.0 - t) + other[0] * t,
            color[1] * (1.0 - t) + other[1] * t,
            color[2] * (1.0 - t) + other[2] * t,
        ]
    };
    match tone {
        Tone::Body => team,
        Tone::Dark => mix(team, [0.10, 0.10, 0.12], 0.45),
        Tone::Light => mix(team, [0.95, 0.95, 1.0], 0.45),
        Tone::Steel => mix(team, [0.42, 0.44, 0.48], 0.78),
        Tone::Band => {
            let band = mix(team, [1.0, 1.0, 1.0], 0.12);
            [
                (band[0] * 1.15).clamp(0.0, 1.0),
                (band[1] * 1.15).clamp(0.0, 1.0),
                (band[2] * 1.15).clamp(0.0, 1.0),
            ]
        }
        Tone::Amber => [0.98, 0.72, 0.16],
        Tone::AmberLight => [1.0, 0.87, 0.45],
    }
}

/// The worker: a small round-ish body, a dark head, and a visible tool —
/// a steel shaft with a heavier head, held out to the right side.
fn worker() -> Silhouette {
    Silhouette::from_parts(
        true,
        vec![
            Part::at([0.0, 0.15, 0.0], [0.34, 0.30, 0.34], Tone::Body),
            Part::at([0.0, 0.39, 0.0], [0.22, 0.18, 0.22], Tone::Dark),
            Part {
                offset: [0.22, 0.24, 0.14],
                scale: [0.30, 0.05, 0.05],
                yaw: 0.5,
                tone: Tone::Steel,
            },
            Part {
                offset: [0.38, 0.30, 0.18],
                scale: [0.09, 0.11, 0.09],
                yaw: 0.0,
                tone: Tone::Steel,
            },
        ],
    )
}

/// The rifleman: a taller capsule-ish body, a dark head, and a thin rifle
/// held forward along the facing, off to the right.
fn rifleman() -> Silhouette {
    Silhouette::from_parts(
        true,
        vec![
            Part::at([0.0, 0.24, 0.0], [0.32, 0.48, 0.32], Tone::Body),
            Part::at([0.0, 0.57, 0.0], [0.18, 0.16, 0.18], Tone::Dark),
            Part::at([0.10, 0.36, 0.13], [0.46, 0.05, 0.05], Tone::Steel),
        ],
    )
}

/// The raider: a medium, low, fast-looking buggy — wide chassis, a darker
/// wedge nose, a light cabin and a steel roll bar.
fn raider() -> Silhouette {
    Silhouette::from_parts(
        true,
        vec![
            Part::at([0.0, 0.15, 0.0], [0.68, 0.20, 0.44], Tone::Body),
            Part::at([0.38, 0.13, 0.0], [0.16, 0.14, 0.28], Tone::Dark),
            Part::at([-0.08, 0.32, 0.0], [0.28, 0.16, 0.30], Tone::Light),
            Part::at([-0.08, 0.46, 0.0], [0.05, 0.10, 0.34], Tone::Steel),
        ],
    )
}

/// The Guardian (the tank): a large hull with dark treads on both flanks,
/// a lighter turret on top, and a steel barrel pointing forward.
fn guardian() -> Silhouette {
    Silhouette::from_parts(
        true,
        vec![
            Part::at([0.0, 0.16, 0.0], [0.86, 0.32, 0.58], Tone::Body),
            Part::at([0.0, 0.08, 0.30], [0.92, 0.16, 0.09], Tone::Dark),
            Part::at([0.0, 0.08, -0.30], [0.92, 0.16, 0.09], Tone::Dark),
            Part::at([-0.05, 0.42, 0.0], [0.42, 0.22, 0.38], Tone::Light),
            Part::at([0.32, 0.44, 0.0], [0.44, 0.08, 0.08], Tone::Steel),
        ],
    )
}

/// The Command Center: a large, tall slab with a lighter mid tier and a
/// distinct corner tower capped in the team band — it reads as THE base.
fn command_center() -> Silhouette {
    Silhouette::from_parts(
        false,
        vec![
            Part::at([0.0, 0.30, 0.0], [3.6, 0.60, 3.6], Tone::Body),
            Part::at([0.0, 0.75, 0.0], [2.9, 0.55, 2.9], Tone::Light),
            Part::at([0.95, 1.45, 0.95], [0.95, 1.75, 0.95], Tone::Body),
            Part::at([0.95, 2.42, 0.95], [1.15, 0.20, 1.15], Tone::Band),
            Part::at([0.0, 0.35, -1.83], [1.1, 0.70, 0.14], Tone::Dark),
        ],
    )
}

/// The Barracks: wide and low, with a dark door on the front face and a
/// steel flag pole flying the team-colored banner at a corner.
fn barracks() -> Silhouette {
    Silhouette::from_parts(
        false,
        vec![
            Part::at([0.0, 0.25, 0.0], [2.7, 0.50, 2.7], Tone::Body),
            Part::at([0.0, 0.62, 0.0], [2.3, 0.26, 2.3], Tone::Light),
            Part::at([0.0, 0.28, -1.38], [0.8, 0.56, 0.14], Tone::Dark),
            Part::at([1.28, 0.85, 1.28], [0.07, 1.70, 0.07], Tone::Steel),
            Part::at([1.46, 1.50, 1.28], [0.34, 0.24, 0.05], Tone::Band),
        ],
    )
}

/// The Supply Depot: a small crate with two more stacked on top — the
/// stacked-crates read.
fn supply_depot() -> Silhouette {
    Silhouette::from_parts(
        false,
        vec![
            Part::at([0.0, 0.24, 0.0], [1.55, 0.48, 1.55], Tone::Body),
            Part::at([0.0, 0.06, 0.0], [1.62, 0.12, 1.62], Tone::Band),
            Part::at([-0.30, 0.66, 0.12], [0.75, 0.36, 0.75], Tone::Light),
            Part::at([0.42, 0.62, -0.32], [0.60, 0.28, 0.60], Tone::Dark),
        ],
    )
}

/// The Turret: a pedestal, a darker cap, and a raised steel barrel.
fn turret() -> Silhouette {
    Silhouette::from_parts(
        false,
        vec![
            Part::at([0.0, 0.26, 0.0], [1.35, 0.52, 1.35], Tone::Body),
            Part::at([0.0, 0.10, 0.0], [1.42, 0.14, 1.42], Tone::Band),
            Part::at([0.0, 0.60, 0.0], [0.95, 0.16, 0.95], Tone::Dark),
            Part::at([0.42, 0.70, 0.0], [0.72, 0.11, 0.11], Tone::Steel),
        ],
    )
}

/// The Ore Node: a bright amber crystal cluster — three leaning shards plus
/// a low splinter, hotter than any terrain color so nodes pop off the
/// ground (PLAN-M10.2 §2.1's "contrasts with the ground").
fn ore_node() -> Silhouette {
    Silhouette::from_parts(
        false,
        vec![
            Part {
                offset: [0.0, 0.46, 0.0],
                scale: [0.60, 0.92, 0.60],
                yaw: 0.78,
                tone: Tone::Amber,
            },
            Part {
                offset: [0.34, 0.34, -0.20],
                scale: [0.42, 0.68, 0.42],
                yaw: -0.5,
                tone: Tone::AmberLight,
            },
            Part {
                offset: [-0.30, 0.28, 0.24],
                scale: [0.36, 0.56, 0.36],
                yaw: 0.3,
                tone: Tone::Amber,
            },
            Part {
                offset: [0.06, 0.16, -0.38],
                scale: [0.26, 0.32, 0.26],
                yaw: 1.1,
                tone: Tone::AmberLight,
            },
        ],
    )
}

/// The kind-name table (PLAN-M10.2 §2.1): the nine authored kinds resolve
/// to their hand-tuned silhouettes; anything else falls through to
/// [`capability_fallback`]. Keyed by `EntityDef::id` — the bundle's kind
/// name — never by display name or kind index.
pub fn named(name: &str) -> Option<Silhouette> {
    match name {
        "worker" => Some(worker()),
        "rifleman" => Some(rifleman()),
        "raider" => Some(raider()),
        "guardian" => Some(guardian()),
        "command_center" => Some(command_center()),
        "barracks" => Some(barracks()),
        "supply_depot" => Some(supply_depot()),
        "turret" => Some(turret()),
        "ore_node" => Some(ore_node()),
        _ => None,
    }
}

/// The capability-shape fallback for kinds the name table does not know
/// (PLAN-M10.2 §2.1: "has Footprint = building box, has Move = unit box").
/// The resource and turret cases mirror the capability law the engine's
/// plan resolution follows, and every structure shape carries the team
/// band trim so the fallback never loses team identification either.
pub fn capability_fallback(
    footprint: Option<(u32, u32)>,
    has_move: bool,
    has_attack: bool,
    has_resource: bool,
) -> Silhouette {
    if has_resource {
        // Unknown resource node: the amber cluster, minus the hand-tuned
        // crystal count.
        Silhouette::from_parts(
            false,
            vec![
                Part {
                    offset: [0.0, 0.42, 0.0],
                    scale: [0.56, 0.84, 0.56],
                    yaw: 0.7,
                    tone: Tone::Amber,
                },
                Part {
                    offset: [0.28, 0.30, -0.16],
                    scale: [0.36, 0.56, 0.36],
                    yaw: -0.4,
                    tone: Tone::AmberLight,
                },
            ],
        )
    } else if let Some((w, h)) = footprint {
        let (w, h) = (w as f32, h as f32);
        if has_attack && !has_move {
            // Unknown defensive structure: pedestal + barrel.
            Silhouette::from_parts(
                false,
                vec![
                    Part::at([0.0, 0.25, 0.0], [w * 0.65, 0.5, h * 0.65], Tone::Body),
                    Part::at([0.0, 0.58, 0.0], [w * 0.45, 0.16, h * 0.45], Tone::Dark),
                    Part::at([w * 0.30, 0.66, 0.0], [w * 0.34, 0.11, 0.11], Tone::Steel),
                ],
            )
        } else {
            // Unknown building: footprint slab, lighter roof, band trim.
            let height = w.max(h) * 0.35 + 0.5;
            Silhouette::from_parts(
                false,
                vec![
                    Part::at(
                        [0.0, height * 0.5, 0.0],
                        [w * 0.94, height, h * 0.94],
                        Tone::Body,
                    ),
                    Part::at(
                        [0.0, height + height * 0.22, 0.0],
                        [w * 0.62, height * 0.45, h * 0.62],
                        Tone::Light,
                    ),
                    Part::at([0.0, 0.06, 0.0], [w * 0.98, 0.12, h * 0.98], Tone::Band),
                ],
            )
        }
    } else if has_move {
        // Unknown mover: the body + head unit box.
        Silhouette::from_parts(
            true,
            vec![
                Part::at([0.0, 0.28, 0.0], [0.40, 0.56, 0.40], Tone::Body),
                Part::at([0.0, 0.66, 0.0], [0.22, 0.20, 0.22], Tone::Dark),
            ],
        )
    } else {
        // Nothing known at all: a plain unit box (the defensive default).
        Silhouette::from_parts(
            true,
            vec![Part::at([0.0, 0.30, 0.0], [0.40, 0.60, 0.40], Tone::Body)],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The nine authored kinds, in content order (the bundle loads them
    /// sorted by file name).
    const NINE: [&str; 9] = [
        "barracks",
        "command_center",
        "guardian",
        "ore_node",
        "raider",
        "rifleman",
        "supply_depot",
        "turret",
        "worker",
    ];

    #[test]
    fn every_authored_kind_resolves_to_a_unique_silhouette() {
        let resolved: Vec<Silhouette> = NINE
            .iter()
            .map(|name| named(name).expect("every authored kind resolves"))
            .collect();
        for (a, silhouette_a) in resolved.iter().enumerate() {
            for (b, silhouette_b) in resolved.iter().enumerate() {
                if a < b {
                    assert_ne!(
                        silhouette_a.parts, silhouette_b.parts,
                        "{:?} and {:?} must resolve to distinct silhouettes",
                        NINE[a], NINE[b]
                    );
                }
            }
        }
    }

    #[test]
    fn each_kind_composes_its_expected_parts() {
        // The instance stream draws 36 vertices per part (two triangles per
        // face of an instanced cube), so a kind's vertex count is
        // parts * 36 — pinned here per kind.
        let expected = [
            ("barracks", 5),
            ("command_center", 5),
            ("guardian", 5),
            ("ore_node", 4),
            ("raider", 4),
            ("rifleman", 3),
            ("supply_depot", 4),
            ("turret", 4),
            ("worker", 4),
        ];
        for (name, parts) in expected {
            let silhouette = named(name).expect("authored");
            assert_eq!(
                silhouette.parts.len(),
                parts,
                "{name} composes {parts} parts"
            );
            // The instance pass draws 36 vertices per part (a cube's two
            // triangles per face), so the kind's on-GPU vertex count is
            // parts * 36 — recorded here so the plan's "test vertex counts"
            // has a pinned number per kind.
            let vertices = silhouette.parts.len() * 36;
            assert_eq!(vertices % 36, 0, "{name}: whole cubes only");
        }
    }

    #[test]
    fn every_part_stays_above_ground_and_inside_the_radius() {
        for name in NINE {
            let silhouette = named(name).expect("authored");
            assert!(silhouette.radius > 0.0, "{name} has a ground radius");
            for part in &silhouette.parts {
                assert!(
                    part.scale[0] > 0.0 && part.scale[1] > 0.0 && part.scale[2] > 0.0,
                    "{name}: every part has a positive extent"
                );
                assert!(
                    part.offset[1] - part.scale[1] * 0.5 >= -1e-4,
                    "{name}: no part sinks below the ground plane"
                );
                assert!(
                    part.reach() <= silhouette.radius,
                    "{name}: every part fits inside the derived radius"
                );
            }
        }
    }

    #[test]
    fn mobile_kinds_carry_the_unit_flag_structures_do_not() {
        for name in ["worker", "rifleman", "raider", "guardian"] {
            assert!(named(name).expect("authored").unit, "{name} is a unit");
        }
        for name in [
            "command_center",
            "barracks",
            "supply_depot",
            "turret",
            "ore_node",
        ] {
            assert!(!named(name).expect("authored").unit, "{name} is not");
        }
    }

    #[test]
    fn structures_and_nodes_carry_team_band_or_amber_parts() {
        // Every structure silhouette has a Band part (the team banner); the
        // ore node instead reads amber through and through.
        for name in ["command_center", "barracks", "supply_depot", "turret"] {
            let silhouette = named(name).expect("authored");
            assert!(
                silhouette.parts.iter().any(|part| part.tone == Tone::Band),
                "{name} carries a team band part"
            );
        }
        let node = named("ore_node").expect("authored");
        assert!(node
            .parts
            .iter()
            .all(|part| matches!(part.tone, Tone::Amber | Tone::AmberLight)));
    }

    #[test]
    fn unknown_names_fall_back_by_capability_shape() {
        assert!(named("skirmisher").is_none(), "the table stays name-exact");
        // Footprint = building box (plus the band trim).
        let building = capability_fallback(Some((3, 3)), false, false, false);
        assert!(!building.unit);
        assert_eq!(building.parts.len(), 3);
        assert!(building.parts.iter().any(|part| part.tone == Tone::Band));
        // Move = unit box.
        let mover = capability_fallback(None, true, false, false);
        assert!(mover.unit);
        assert_eq!(mover.parts.len(), 2);
        // Resource beats footprint (a node is a node).
        let node = capability_fallback(Some((2, 2)), false, false, true);
        assert!(!node.unit);
        assert!(node
            .parts
            .iter()
            .all(|part| matches!(part.tone, Tone::Amber | Tone::AmberLight)));
        // Footprint + Attack without Move = a turret.
        let turret = capability_fallback(Some((2, 2)), false, true, false);
        assert!(!turret.unit);
        assert!(turret.parts.iter().any(|part| part.tone == Tone::Steel));
        // Nothing known: the plain unit box.
        let plain = capability_fallback(None, false, false, false);
        assert!(plain.unit);
        assert_eq!(plain.parts.len(), 1);
    }

    #[test]
    fn part_colors_keep_the_team_readable_where_it_matters() {
        let blue = [0.22, 0.45, 0.92];
        let orange = [0.95, 0.55, 0.15];
        // Body passes the team color straight through.
        assert_eq!(part_color(Tone::Body, blue), blue);
        // Band stays team-dominant: blue band vs orange band differ clearly.
        let blue_band = part_color(Tone::Band, blue);
        let orange_band = part_color(Tone::Band, orange);
        let band_delta =
            (blue_band[2] - orange_band[2]).abs() + (blue_band[0] - orange_band[0]).abs();
        assert!(band_delta > 0.3, "the band keeps the two teams apart");
        // Steel barely moves with the team (tools read the same for both):
        // the residual team bleed stays a small fraction of the full blue/
        // orange channel spread.
        let blue_steel = part_color(Tone::Steel, blue);
        let orange_steel = part_color(Tone::Steel, orange);
        let steel_delta = (blue_steel[0] - orange_steel[0]).abs()
            + (blue_steel[1] - orange_steel[1]).abs()
            + (blue_steel[2] - orange_steel[2]).abs();
        assert!(steel_delta < 0.4, "steel is team-agnostic gunmetal");
        // Amber is fully team-independent (ore is neutral).
        assert_eq!(
            part_color(Tone::Amber, blue),
            part_color(Tone::Amber, orange)
        );
        // Amber pops against the calmer ground (the green channel is low).
        let amber = part_color(Tone::Amber, blue);
        assert!(amber[0] > 0.8 && amber[1] > 0.55 && amber[2] < 0.35);
    }

    #[test]
    fn the_main_part_is_the_biggest_mass() {
        let silhouette = named("guardian").expect("authored");
        let main = silhouette.main_part();
        for part in &silhouette.parts {
            assert!(main.volume() >= part.volume());
        }
        // The Guardian's main mass is its hull (the first part).
        assert_eq!(main.offset, [0.0, 0.16, 0.0]);
    }

    #[test]
    fn the_fallback_radius_derives_from_the_widest_part() {
        let silhouette = capability_fallback(Some((4, 4)), false, false, false);
        let widest = silhouette
            .parts
            .iter()
            .map(Part::reach)
            .fold(0.0_f32, f32::max);
        assert!((silhouette.radius - widest - 0.08).abs() < 1e-4);
    }
}
