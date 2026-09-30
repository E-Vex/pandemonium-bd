//! Semantic validation (plan §14 M2: "malformed files produce precise errors").
//!
//! Two levels:
//!
//! - **File level** — checks inside one file that need no cross-references
//!   beyond the entity table: rules shape, entity stats, faction references,
//!   map dimensions/grid/legend/starts/nodes/heightmap.
//! - **Placement level** — the cross of a map with a faction (plan §10.5):
//!   spawn legality (footprints in bounds, on buildable ground, non-overlapping),
//!   ore reachability (BFS from each start), and the optional symmetry check
//!   (run when the map declares `symmetric: true`).
//!
//! Every rejection names its file, entity/map, coordinates, and the exact rule
//! violated — a designer should never have to guess which line is wrong.

use crate::defs::{CapabilityDef, EntityDef, FactionDef, MapDef, RulesDef};
use crate::error::ContentError;

/// The largest map dimension accepted (a sanity cap for authoring mistakes; the
/// Alpha map is 64×64 — plan §10.5). Recorded as an assumption.
const MAX_MAP_DIMENSION: u32 = 4096;

/// The neutral player sentinel may not own a start position.
const NEUTRAL_PLAYER: u8 = u8::MAX;

// ---------------------------------------------------------------------------
// Rules
// ---------------------------------------------------------------------------

/// Validates the canonical rules: at least one resource, unique non-empty ids,
/// non-negative starting balances.
pub(crate) fn validate_rules(file: &str, rules: &RulesDef) -> Result<(), ContentError> {
    if rules.resources.is_empty() {
        return Err(ContentError::InvalidRules {
            file: file.to_string(),
            detail:
                "the resource registry is empty (at least one resource is required — plan §9.3)"
                    .to_string(),
        });
    }
    for (index, resource) in rules.resources.iter().enumerate() {
        if resource.id.is_empty() {
            return Err(ContentError::InvalidRules {
                file: file.to_string(),
                detail: format!("resources[{index}] has an empty id"),
            });
        }
        if resource.starting < 0 {
            return Err(ContentError::InvalidRules {
                file: file.to_string(),
                detail: format!(
                    "resource '{}' has a negative starting balance ({})",
                    resource.id, resource.starting
                ),
            });
        }
        for later in &rules.resources[index + 1..] {
            if later.id == resource.id {
                return Err(ContentError::InvalidRules {
                    file: file.to_string(),
                    detail: format!("duplicate resource id '{}'", resource.id),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

/// Validates one canonical entity: no duplicate capabilities, every stat in
/// range, resource references resolved (plan §10.2 authoring units).
pub(crate) fn validate_entity(
    file: &str,
    entity: &EntityDef,
    rules: &RulesDef,
) -> Result<(), ContentError> {
    let id = &entity.id;
    for (index, cap) in entity.capabilities.iter().enumerate() {
        for later in &entity.capabilities[index + 1..] {
            if later.name() == cap.name() {
                return Err(ContentError::DuplicateCapability {
                    file: file.to_string(),
                    entity: id.clone(),
                    capability: cap.name().to_string(),
                });
            }
        }
    }
    let stat = |field: &str, value: i64, why: &str| -> ContentError {
        ContentError::InvalidStat {
            file: file.to_string(),
            entity: id.clone(),
            field: field.to_string(),
            value,
            why: why.to_string(),
        }
    };
    for cap in &entity.capabilities {
        match cap {
            CapabilityDef::Health { max_hp, .. } => {
                if *max_hp <= 0 {
                    return Err(stat("Health.max", *max_hp as i64, "must be positive"));
                }
            }
            CapabilityDef::Move {
                speed_milli_tiles_per_s,
                radius_milli_tiles,
            } => {
                if *speed_milli_tiles_per_s < 0 {
                    return Err(stat(
                        "Move.speed_milli_tiles_per_s",
                        *speed_milli_tiles_per_s as i64,
                        "must not be negative",
                    ));
                }
                if *radius_milli_tiles < 0 {
                    return Err(stat(
                        "Move.radius_milli_tiles",
                        *radius_milli_tiles as i64,
                        "must not be negative",
                    ));
                }
            }
            CapabilityDef::Attack {
                damage,
                range_milli_tiles,
                acquire_range_milli_tiles,
                ..
            } => {
                if *damage <= 0 {
                    return Err(stat("Attack.damage", *damage as i64, "must be positive"));
                }
                if *range_milli_tiles <= 0 {
                    return Err(stat(
                        "Attack.range_milli_tiles",
                        *range_milli_tiles as i64,
                        "must be positive",
                    ));
                }
                if *acquire_range_milli_tiles < *range_milli_tiles {
                    return Err(stat(
                        "Attack.acquire_range_milli_tiles",
                        *acquire_range_milli_tiles as i64,
                        "must be at least range_milli_tiles (a targetable range \
                         shorter than the attack range can never acquire)",
                    ));
                }
            }
            CapabilityDef::Gather {
                carry_amount,
                gather_time_ms,
                ..
            } => {
                if *carry_amount <= 0 {
                    return Err(stat(
                        "Gather.carry_amount",
                        *carry_amount,
                        "must be positive",
                    ));
                }
                if *gather_time_ms == 0 {
                    return Err(stat(
                        "Gather.gather_time_ms",
                        0,
                        "must be positive (a zero gather time never completes a cycle)",
                    ));
                }
            }
            CapabilityDef::ProvidesPopulation { amount } => {
                if *amount <= 0 {
                    return Err(stat(
                        "ProvidesPopulation.amount",
                        *amount as i64,
                        "must be positive",
                    ));
                }
            }
            CapabilityDef::Resource { resource, amount } => {
                if *amount <= 0 {
                    return Err(stat("Resource.amount", *amount, "must be positive"));
                }
                if !rules.resources.iter().any(|res| &res.id == resource) {
                    return Err(ContentError::UnknownResourceRef {
                        file: file.to_string(),
                        what: format!("entity '{id}' Resource capability"),
                        reference: resource.clone(),
                        registered: rules
                            .resources
                            .iter()
                            .map(|res| res.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", "),
                    });
                }
            }
            CapabilityDef::Vision { radius_milli_tiles } => {
                if *radius_milli_tiles <= 0 {
                    return Err(stat(
                        "Vision.radius_milli_tiles",
                        *radius_milli_tiles as i64,
                        "must be positive",
                    ));
                }
            }
            CapabilityDef::Footprint { w, h } => {
                if *w == 0 || *h == 0 {
                    return Err(stat(
                        "Footprint.w/h",
                        0,
                        "both dimensions must be at least 1 tile",
                    ));
                }
            }
            CapabilityDef::Storage { resources } => {
                for resource in resources {
                    if !rules.resources.iter().any(|res| &res.id == resource) {
                        return Err(ContentError::UnknownResourceRef {
                            file: file.to_string(),
                            what: format!("entity '{id}' Storage capability"),
                            reference: resource.clone(),
                            registered: rules
                                .resources
                                .iter()
                                .map(|res| res.id.as_str())
                                .collect::<Vec<_>>()
                                .join(", "),
                        });
                    }
                }
            }
            CapabilityDef::Build | CapabilityDef::Produce => {}
        }
    }
    if entity.cost_ore < 0 {
        return Err(stat("cost.ore", entity.cost_ore, "must not be negative"));
    }
    if entity.population < 0 {
        return Err(stat(
            "population",
            entity.population as i64,
            "must not be negative",
        ));
    }
    Ok(())
}

/// Validates that every `requires` entry references a loaded entity (runs once
/// all entities are known).
pub(crate) fn validate_entity_references(entities: &[EntityDef]) -> Result<(), ContentError> {
    for entity in entities {
        for requirement in &entity.requires {
            if !entities.iter().any(|other| &other.id == requirement) {
                return Err(ContentError::UnknownEntityRef {
                    file: entity.file.clone(),
                    what: format!("entity '{}' requires", entity.id),
                    reference: requirement.clone(),
                });
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Factions
// ---------------------------------------------------------------------------

/// Validates one canonical faction: roster/production/starting-force references
/// resolve, producers actually produce, starting forces exist and are not
/// resource nodes.
pub(crate) fn validate_faction(
    file: &str,
    faction: &FactionDef,
    entities: &[EntityDef],
) -> Result<(), ContentError> {
    let find = |id: &str| entities.iter().find(|entity| entity.id == id);
    if faction.starting_forces.is_empty() {
        return Err(ContentError::InvalidFaction {
            file: file.to_string(),
            faction: faction.id.clone(),
            detail: "starting_forces is empty (every faction needs at least one \
                     starting entity — plan §9.7)"
                .to_string(),
        });
    }
    for member in &faction.roster {
        if find(member).is_none() {
            return Err(ContentError::UnknownEntityRef {
                file: file.to_string(),
                what: format!("faction '{}' roster", faction.id),
                reference: member.clone(),
            });
        }
    }
    for (producer, producibles) in &faction.production {
        if !faction.roster.contains(producer) {
            return Err(ContentError::InvalidFaction {
                file: file.to_string(),
                faction: faction.id.clone(),
                detail: format!(
                    "production lists producer '{producer}' which is not in the roster"
                ),
            });
        }
        let Some(producer_def) = find(producer) else {
            return Err(ContentError::UnknownEntityRef {
                file: file.to_string(),
                what: format!("faction '{}' production list", faction.id),
                reference: producer.clone(),
            });
        };
        if producer_def.capability("Produce").is_none() {
            return Err(ContentError::InvalidFaction {
                file: file.to_string(),
                faction: faction.id.clone(),
                detail: format!(
                    "production lists producer '{producer}' which does not carry the \
                     Produce capability"
                ),
            });
        }
        for producible in producibles {
            if find(producible).is_none() {
                return Err(ContentError::UnknownEntityRef {
                    file: file.to_string(),
                    what: format!("faction '{}' production of '{producer}'", faction.id),
                    reference: producible.clone(),
                });
            }
            if !faction.roster.contains(producible) {
                return Err(ContentError::InvalidFaction {
                    file: file.to_string(),
                    faction: faction.id.clone(),
                    detail: format!("production trains '{producible}' which is not in the roster"),
                });
            }
        }
    }
    for force in &faction.starting_forces {
        let Some(force_def) = find(&force.entity) else {
            return Err(ContentError::UnknownEntityRef {
                file: file.to_string(),
                what: format!("faction '{}' starting forces", faction.id),
                reference: force.entity.clone(),
            });
        };
        if !faction.roster.contains(&force.entity) {
            return Err(ContentError::InvalidFaction {
                file: file.to_string(),
                faction: faction.id.clone(),
                detail: format!("starting force '{}' is not in the roster", force.entity),
            });
        }
        if force_def.capability("Resource").is_some() {
            return Err(ContentError::InvalidFaction {
                file: file.to_string(),
                faction: faction.id.clone(),
                detail: format!(
                    "starting force '{}' is a resource node — nodes are placed by maps, \
                     not by starting forces (plan §10.5)",
                    force.entity
                ),
            });
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Maps — file level
// ---------------------------------------------------------------------------

/// Validates one canonical map's shape: dimensions, legend, grid, starts, ore
/// node references and footprint fit, heightmap dimensions.
pub(crate) fn validate_map_file(
    file: &str,
    map: &MapDef,
    entities: &[EntityDef],
) -> Result<(), ContentError> {
    let invalid = |detail: String| ContentError::InvalidMap {
        file: file.to_string(),
        map: map.id.clone(),
        detail,
    };

    if map.width == 0 || map.height == 0 {
        return Err(invalid(format!(
            "dimensions are {}x{} (both must be at least 1)",
            map.width, map.height
        )));
    }
    if map.width > MAX_MAP_DIMENSION || map.height > MAX_MAP_DIMENSION {
        return Err(invalid(format!(
            "dimensions are {}x{} (both must be at most {MAX_MAP_DIMENSION})",
            map.width, map.height
        )));
    }
    if map.terrain.is_empty() {
        return Err(invalid("the terrain legend is empty".to_string()));
    }
    for (index, class) in map.terrain.iter().enumerate() {
        for later in &map.terrain[index + 1..] {
            if later.code == class.code {
                return Err(invalid(format!(
                    "terrain code '{}' is declared twice ({} and {})",
                    class.code, class.name, later.name
                )));
            }
        }
    }
    if map.grid.len() != map.height as usize {
        return Err(invalid(format!(
            "grid has {} rows, expected {}",
            map.grid.len(),
            map.height
        )));
    }
    for (row_index, row) in map.grid.iter().enumerate() {
        if row.chars().count() != map.width as usize {
            return Err(invalid(format!(
                "grid row {} has {} characters, expected {}",
                row_index,
                row.chars().count(),
                map.width
            )));
        }
        for (column_index, code) in row.chars().enumerate() {
            if map.terrain_class(code).is_none() {
                return Err(invalid(format!(
                    "grid row {row_index} column {column_index} uses code '{code}' \
                     which is not in the terrain legend"
                )));
            }
        }
    }
    if map.starts.is_empty() {
        return Err(invalid(
            "there are no start positions (at least one is required — plan §10.5)".to_string(),
        ));
    }
    for (index, start) in map.starts.iter().enumerate() {
        for later in &map.starts[index + 1..] {
            if later.player == start.player {
                return Err(invalid(format!(
                    "player {} owns more than one start position",
                    start.player
                )));
            }
        }
        if start.player == NEUTRAL_PLAYER {
            return Err(invalid(
                "a start position uses the neutral player sentinel".to_string(),
            ));
        }
        if start.x < 0 || start.y < 0 || start.x >= map.width as i32 || start.y >= map.height as i32
        {
            return Err(invalid(format!(
                "start for player {} has anchor ({},{}) outside the {}x{} map",
                start.player, start.x, start.y, map.width, map.height
            )));
        }
    }
    if map.ore_nodes.is_empty() {
        return Err(invalid(
            "there are no ore nodes (plan §10.5: every start needs reachable ore)".to_string(),
        ));
    }
    for (index, node) in map.ore_nodes.iter().enumerate() {
        let Some(node_def) = entities.iter().find(|entity| entity.id == node.kind) else {
            return Err(ContentError::UnknownEntityRef {
                file: file.to_string(),
                what: format!("map '{}' ore_nodes[{index}]", map.id),
                reference: node.kind.clone(),
            });
        };
        if node_def.capability("Resource").is_none() || node_def.footprint().is_none() {
            return Err(invalid(format!(
                "ore_nodes[{index}] uses kind '{}' which must carry both the Resource \
                 and Footprint capabilities",
                node.kind
            )));
        }
        let (w, h) = node_def.footprint().expect("footprint checked above");
        if node.x < 0
            || node.y < 0
            || node.x + w as i32 > map.width as i32
            || node.y + h as i32 > map.height as i32
        {
            return Err(invalid(format!(
                "ore_nodes[{index}] ('{}') at ({},{}) with footprint {w}x{h} extends \
                 outside the {}x{} map",
                node.kind, node.x, node.y, map.width, map.height
            )));
        }
        for later in &map.ore_nodes[index + 1..] {
            let (lw, lh) = entities
                .iter()
                .find(|entity| entity.id == later.kind)
                .and_then(|entity| entity.footprint())
                .unwrap_or((1, 1));
            let overlap_x = node.x < later.x + lw as i32 && later.x < node.x + w as i32;
            let overlap_y = node.y < later.y + lh as i32 && later.y < node.y + h as i32;
            if overlap_x && overlap_y {
                return Err(invalid(format!(
                    "ore_nodes[{index}] at ({},{}) overlaps another ore node at ({},{})",
                    node.x, node.y, later.x, later.y
                )));
            }
        }
    }
    if let Some(heightmap) = &map.heightmap {
        if heightmap.rows.len() != map.height as usize {
            return Err(invalid(format!(
                "heightmap has {} rows, expected {}",
                heightmap.rows.len(),
                map.height
            )));
        }
        for (row_index, row) in heightmap.rows.iter().enumerate() {
            if row.len() != map.width as usize {
                return Err(invalid(format!(
                    "heightmap row {row_index} has {} entries, expected {}",
                    row.len(),
                    map.width
                )));
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Maps — placement level (per map x faction, plan §10.5)
// ---------------------------------------------------------------------------

/// One statically occupied tile: what claims it (for precise overlap errors).
struct Occupant {
    /// The tile.
    x: i32,
    /// The tile y.
    y: i32,
    /// What claims it ("ore_node at (18,10)", "command_center of player 0").
    what: String,
}

impl Occupant {
    fn covers(&self, x: i32, y: i32) -> bool {
        self.x == x && self.y == y
    }
}

/// Finds what statically claims a tile, if anything.
fn find_occupant(occupants: &[Occupant], x: i32, y: i32) -> Option<&Occupant> {
    occupants.iter().find(|occ| occ.covers(x, y))
}

/// Validates one (map, faction) pair's placements: every start's forces fit
/// legally, every ore node is reachable from every start, and — when the map
/// declares symmetry — the rotational symmetry check holds.
pub(crate) fn validate_map_placements(
    map: &MapDef,
    faction: &FactionDef,
    entities: &[EntityDef],
) -> Result<(), ContentError> {
    let find = |id: &str| entities.iter().find(|entity| entity.id == id);

    // Static occupancy starts with the ore nodes (every faction must coexist
    // with them).
    let mut occupants: Vec<Occupant> = Vec::new();
    for node in &map.ore_nodes {
        let (w, h) = find(&node.kind)
            .and_then(|def| def.footprint())
            .unwrap_or((1, 1));
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                occupants.push(Occupant {
                    x: node.x + dx,
                    y: node.y + dy,
                    what: format!("ore node '{}' at ({},{})", node.kind, node.x, node.y),
                });
            }
        }
    }

    // Spawn legality, start by start (authored order), accumulating occupancy.
    for start in &map.starts {
        for force in &faction.starting_forces {
            let Some(force_def) = find(&force.entity) else {
                // Reference validity is the faction validator's job; skip here.
                continue;
            };
            let tile_x = start.x + force.offset.0;
            let tile_y = start.y + force.offset.1;
            match force_def.footprint() {
                Some((w, h)) => {
                    // A structure: every footprint tile in bounds, buildable,
                    // and unclaimed.
                    for dy in 0..h as i32 {
                        for dx in 0..w as i32 {
                            let x = tile_x + dx;
                            let y = tile_y + dy;
                            let illegal = |detail: String| ContentError::IllegalStart {
                                file: map.file.clone(),
                                map: map.id.clone(),
                                player: start.player,
                                detail,
                            };
                            if x < 0 || y < 0 || x >= map.width as i32 || y >= map.height as i32 {
                                return Err(illegal(format!(
                                    "'{}' at ({tile_x},{tile_y}) with footprint {w}x{h} \
                                     extends outside the {}x{} map",
                                    force.entity, map.width, map.height
                                )));
                            }
                            if !map.buildable(x, y) {
                                return Err(illegal(format!(
                                    "'{}' footprint tile ({x},{y}) sits on terrain '{}' \
                                     which is not buildable",
                                    force.entity,
                                    map.terrain_at(x, y).map(|t| t.name.as_str()).unwrap_or("?")
                                )));
                            }
                            if let Some(occupant) = find_occupant(&occupants, x, y) {
                                return Err(illegal(format!(
                                    "'{}' footprint tile ({x},{y}) is already occupied by {}",
                                    force.entity, occupant.what
                                )));
                            }
                        }
                    }
                    for dy in 0..h as i32 {
                        for dx in 0..w as i32 {
                            occupants.push(Occupant {
                                x: tile_x + dx,
                                y: tile_y + dy,
                                what: format!(
                                    "'{}' of player {} (at ({tile_x},{tile_y}))",
                                    force.entity, start.player
                                ),
                            });
                        }
                    }
                }
                None => {
                    // A unit: its spawn tile must be in bounds, passable, and
                    // not inside a static footprint.
                    let illegal = |detail: String| ContentError::IllegalStart {
                        file: map.file.clone(),
                        map: map.id.clone(),
                        player: start.player,
                        detail,
                    };
                    if tile_x < 0
                        || tile_y < 0
                        || tile_x >= map.width as i32
                        || tile_y >= map.height as i32
                    {
                        return Err(illegal(format!(
                            "unit '{}' spawns at ({tile_x},{tile_y}) outside the {}x{} map",
                            force.entity, map.width, map.height
                        )));
                    }
                    if !map.passable(tile_x, tile_y) {
                        return Err(illegal(format!(
                            "unit '{}' spawns at ({tile_x},{tile_y}) on impassable terrain '{}'",
                            force.entity,
                            map.terrain_at(tile_x, tile_y)
                                .map(|t| t.name.as_str())
                                .unwrap_or("?")
                        )));
                    }
                    if let Some(occupant) = find_occupant(&occupants, tile_x, tile_y) {
                        return Err(illegal(format!(
                            "unit '{}' spawns at ({tile_x},{tile_y}) inside {}",
                            force.entity, occupant.what
                        )));
                    }
                }
            }
        }
    }

    // Ore reachability: BFS from each start over free, passable tiles; every
    // ore node must be reachable (its adjacency ring intersected with the
    // reached set must be non-empty).
    for start in &map.starts {
        let reached = reachability_from(map, &occupants, start, faction, entities);
        for node in &map.ore_nodes {
            let (w, h) = find(&node.kind)
                .and_then(|def| def.footprint())
                .unwrap_or((1, 1));
            let mut reachable = false;
            'ring: for ring_y in node.y - 1..=node.y + h as i32 {
                for ring_x in node.x - 1..=node.x + w as i32 {
                    let inside_footprint = ring_x >= node.x
                        && ring_x < node.x + w as i32
                        && ring_y >= node.y
                        && ring_y < node.y + h as i32;
                    if !inside_footprint
                        && reached
                            .get(ring_y as usize)
                            .is_some_and(|row| *row.get(ring_x as usize).unwrap_or(&false))
                    {
                        reachable = true;
                        break 'ring;
                    }
                }
            }
            if !reachable {
                return Err(ContentError::OreUnreachable {
                    file: map.file.clone(),
                    map: map.id.clone(),
                    kind: node.kind.clone(),
                    x: node.x,
                    y: node.y,
                    player: start.player,
                });
            }
        }
    }

    // The optional symmetry check (plan §10.5): run exactly when declared.
    if map.symmetric {
        validate_symmetry(map, faction, entities)?;
    }
    Ok(())
}

/// BFS over free (passable, unoccupied) tiles from one start's structures.
/// The seeds are the free tiles adjacent to the start's footprinted forces; a
/// start with no structures seeds from its anchor tile. Returns a reached-flag
/// grid.
fn reachability_from(
    map: &MapDef,
    occupants: &[Occupant],
    start: &crate::defs::StartDef,
    faction: &FactionDef,
    entities: &[EntityDef],
) -> Vec<Vec<bool>> {
    let width = map.width as usize;
    let height = map.height as usize;
    let mut free = vec![vec![false; width]; height];
    for (y, row) in free.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            *cell = map.passable(x as i32, y as i32)
                && !occupants.iter().any(|occ| occ.covers(x as i32, y as i32));
        }
    }
    let mut seeds: Vec<(i32, i32)> = Vec::new();
    for force in &faction.starting_forces {
        let Some(force_def) = entities.iter().find(|entity| entity.id == force.entity) else {
            continue;
        };
        let tile_x = start.x + force.offset.0;
        let tile_y = start.y + force.offset.1;
        if let Some((w, h)) = force_def.footprint() {
            for ring_y in tile_y - 1..=tile_y + h as i32 {
                for ring_x in tile_x - 1..=tile_x + w as i32 {
                    if ring_x >= 0
                        && ring_y >= 0
                        && ring_x < width as i32
                        && ring_y < height as i32
                        && free[ring_y as usize][ring_x as usize]
                    {
                        seeds.push((ring_x, ring_y));
                    }
                }
            }
        }
    }
    if seeds.is_empty() {
        // A start with no footprinted forces seeds from its anchor tile.
        if start.x >= 0
            && start.y >= 0
            && start.x < width as i32
            && start.y < height as i32
            && free[start.y as usize][start.x as usize]
        {
            seeds.push((start.x, start.y));
        }
    }

    let mut reached = vec![vec![false; width]; height];
    let mut queue = std::collections::VecDeque::new();
    for &(x, y) in &seeds {
        if !reached[y as usize][x as usize] {
            reached[y as usize][x as usize] = true;
            queue.push_back((x, y));
        }
    }
    while let Some((x, y)) = queue.pop_front() {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let nx = x + dx;
                let ny = y + dy;
                if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                    continue;
                }
                if !free[ny as usize][nx as usize] || reached[ny as usize][nx as usize] {
                    continue;
                }
                reached[ny as usize][nx as usize] = true;
                queue.push_back((nx, ny));
            }
        }
    }
    reached
}

/// The rotational (180-degree) symmetry check (plan §10.5, optional validator):
/// the terrain grid, the ore node footprint tile multiset, and the start
/// anchors (mirrored through each anchor's primary structure footprint) must
/// each map onto themselves.
fn validate_symmetry(
    map: &MapDef,
    faction: &FactionDef,
    entities: &[EntityDef],
) -> Result<(), ContentError> {
    let broken = |detail: String| ContentError::SymmetryBroken {
        file: map.file.clone(),
        map: map.id.clone(),
        detail,
    };
    let width = map.width as i32;
    let height = map.height as i32;

    // Terrain grid.
    for y in 0..height {
        for x in 0..width {
            let here = map.grid[y as usize].chars().nth(x as usize).unwrap_or('?');
            let mx = width - 1 - x;
            let my = height - 1 - y;
            let mirrored = map.grid[my as usize]
                .chars()
                .nth(mx as usize)
                .unwrap_or('?');
            if here != mirrored {
                return Err(broken(format!(
                    "terrain at ({x},{y}) is '{here}' while its rotation ({mx},{my}) is \
                     '{mirrored}'"
                )));
            }
        }
    }

    // Ore node footprint tile multiset vs its rotation.
    let mut node_tiles: Vec<(i32, i32)> = Vec::new();
    for node in &map.ore_nodes {
        let (w, h) = entities
            .iter()
            .find(|entity| entity.id == node.kind)
            .and_then(|def| def.footprint())
            .unwrap_or((1, 1));
        for dy in 0..h as i32 {
            for dx in 0..w as i32 {
                node_tiles.push((node.x + dx, node.y + dy));
            }
        }
    }
    let mut mirrored_tiles: Vec<(i32, i32)> = node_tiles
        .iter()
        .map(|&(x, y)| (width - 1 - x, height - 1 - y))
        .collect();
    node_tiles.sort();
    mirrored_tiles.sort();
    if node_tiles != mirrored_tiles {
        return Err(broken(
            "the ore node layout does not map onto itself under 180-degree rotation".to_string(),
        ));
    }

    // Start anchors: the mirror of each anchor (adjusted by the footprint of
    // the faction's first footprinted starting force) must be another anchor.
    let anchor_footprint = faction
        .starting_forces
        .iter()
        .find_map(|force| {
            entities
                .iter()
                .find(|entity| entity.id == force.entity)
                .and_then(|def| def.footprint())
        })
        .unwrap_or((1, 1));
    let mut mirrored_anchors: Vec<(i32, i32)> = map
        .starts
        .iter()
        .map(|start| {
            (
                width - start.x - anchor_footprint.0 as i32,
                height - start.y - anchor_footprint.1 as i32,
            )
        })
        .collect();
    let mut anchors: Vec<(i32, i32)> = map.starts.iter().map(|s| (s.x, s.y)).collect();
    anchors.sort();
    mirrored_anchors.sort();
    if anchors != mirrored_anchors {
        return Err(broken(
            "the start anchors do not map onto each other under 180-degree rotation".to_string(),
        ));
    }
    Ok(())
}
