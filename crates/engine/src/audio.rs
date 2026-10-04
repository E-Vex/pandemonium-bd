//! Placeholder audio cues (plan §11.5, M9): an [`AudioSink`] trait fed by
//! simulation events, a null implementation for headless runs and tests, and
//! a pure event → cue mapping.
//!
//! Plan §11.5's Alpha shape: "an AudioSink trait fed by events, with a null
//! implementation and optionally a few placeholder cues." The cues ARE the
//! placeholder audio — named feedback moments a real sink (a mixer, post
//! Alpha, per plan §17: "rewritten on purpose over time: renderer, UI skin,
//! audio") would voice. No audio backend exists yet and none is needed: the
//! seam exists so the client can wire cues today and swap the sink later
//! without touching anything above the boundary.
//!
//! Determinism posture (FD-9, plan §5): events flow outward — the sink is
//! fed from the presentation layer after the step, never from inside the
//! simulation, and produces no state the simulation could observe. The
//! mapping is a pure function of an event; the null sink is a no-op.

use pandemonium_sim_api::Event;

/// A placeholder audio cue: one named feedback moment. The variants are the
/// Alpha's "few placeholder cues" — combat, loss, production, economy, and
/// the match end, the moments a player needs to hear even while looking
/// elsewhere.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioCue {
    /// An attack landed (`AttackHit`) — the combat heartbeat.
    AttackLanded,
    /// An entity died (`Died`).
    UnitLost,
    /// A production item completed (`ProductionCompleted`).
    UnitReady,
    /// A structure finished constructing (`ConstructionCompleted`).
    StructureDone,
    /// A worker delivered resources (`ResourceDelivered`).
    Delivery,
    /// The match ended (`MatchEnded`) — victory or defeat.
    MatchEnded,
}

/// The pure event → cue mapping (plan §11.5's "fed by events"). Every cue a
/// real sink would voice, derived from exactly the events the boundary
/// already emits — no new event variants, no sim changes. Events with no
/// audio moment (spawn bookkeeping, rejections, path failures, depletion)
/// map to `None`.
pub fn cue_for(event: &Event) -> Option<AudioCue> {
    match event {
        Event::AttackHit { .. } => Some(AudioCue::AttackLanded),
        Event::Died { .. } => Some(AudioCue::UnitLost),
        Event::ProductionCompleted { .. } => Some(AudioCue::UnitReady),
        Event::ConstructionCompleted { .. } => Some(AudioCue::StructureDone),
        Event::ResourceDelivered { .. } => Some(AudioCue::Delivery),
        Event::MatchEnded { .. } => Some(AudioCue::MatchEnded),
        Event::Spawned { .. }
        | Event::ProductionStarted { .. }
        | Event::ConstructionStarted { .. }
        | Event::NodeDepleted { .. }
        | Event::CommandRejected { .. }
        | Event::MoveFailed { .. } => None,
    }
}

/// An audio sink: receives simulation events after a step, voices the cues
/// it maps. Implementations are presentation-only (FD-9) — they may keep
/// counters, log, mix, or do nothing at all; they must never feed anything
/// back into the simulation.
pub trait AudioSink {
    /// Feed one step's events (in emission order).
    fn on_events(&mut self, events: &[Event]);
}

/// The headless sink: accepts and discards events. Exists so tests, soak
/// runs, and CI exercise exactly the path the windowed client feeds (the
/// same shape as the null renderer, plan §11.2).
#[derive(Default)]
pub struct NullAudioSink {
    /// How many events it has been fed (the smoke summary's evidence that
    /// the wiring ran — a counter, not audio).
    pub fed: usize,
    /// How many of those mapped to cues.
    pub cues: usize,
}

impl NullAudioSink {
    /// Builds the counter sink (zero fed, zero cues).
    pub fn new() -> Self {
        Self::default()
    }
}

impl AudioSink for NullAudioSink {
    fn on_events(&mut self, events: &[Event]) {
        self.fed += events.len();
        self.cues += events.iter().filter_map(cue_for).count();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pandemonium_sim_api::{EntityId, PlayerId};

    fn hit() -> Event {
        Event::AttackHit {
            attacker: EntityId(1),
            target: EntityId(2),
            damage: 8,
        }
    }

    fn died() -> Event {
        Event::Died {
            entity: EntityId(2),
        }
    }

    fn delivered() -> Event {
        Event::ResourceDelivered {
            worker: EntityId(3),
            resource: pandemonium_sim_api::ResourceId(0),
            amount: 10,
        }
    }
    fn rejected() -> Event {
        Event::CommandRejected {
            issuer: PlayerId(0),
            seq: 1,
            reject: pandemonium_sim_api::Reject {
                reason: pandemonium_sim_api::RejectReason::MissingCapability,
            },
        }
    }

    fn match_ended() -> Event {
        Event::MatchEnded {
            winner: PlayerId(0),
        }
    }

    #[test]
    fn every_cue_moment_maps_and_the_rest_stay_silent() {
        // The Alpha's placeholder cue set: combat, loss, production,
        // construction, economy, and the match end.
        assert_eq!(cue_for(&hit()), Some(AudioCue::AttackLanded));
        assert_eq!(cue_for(&died()), Some(AudioCue::UnitLost));
        assert_eq!(cue_for(&delivered()), Some(AudioCue::Delivery));
        assert_eq!(cue_for(&match_ended()), Some(AudioCue::MatchEnded));
        // Bookkeeping and failure-path events carry no audio moment.
        assert_eq!(cue_for(&rejected()), None);
    }

    #[test]
    fn production_and_construction_map_to_their_cues() {
        let production = Event::ProductionCompleted {
            producer: EntityId(9),
            producible: pandemonium_sim_api::KindId(5),
        };
        let construction = Event::ConstructionCompleted { site: EntityId(28) };
        assert_eq!(cue_for(&production), Some(AudioCue::UnitReady));
        assert_eq!(cue_for(&construction), Some(AudioCue::StructureDone));
    }

    #[test]
    fn the_null_sink_counts_what_it_is_fed() {
        // The wiring evidence for headless smoke runs: fed and cue counters
        // advance, nothing panics, no state exists beyond the counters.
        let mut sink = NullAudioSink::new();
        assert_eq!((sink.fed, sink.cues), (0, 0));
        sink.on_events(&[hit(), died()]);
        assert_eq!((sink.fed, sink.cues), (2, 2));
        sink.on_events(&[rejected()]);
        assert_eq!((sink.fed, sink.cues), (3, 2));
        sink.on_events(&[]);
        assert_eq!((sink.fed, sink.cues), (3, 2));
    }
}
