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
//!
//! M10.2 Phase 4 (PLAN §4.2) widened the cue vocabulary: three client-side
//! moments with no sim event at all (command acknowledged, selection click,
//! UI click) join the enum, fed directly through [`AudioSink::on_cue`] from
//! the wiring layer — the event → cue mapping stays untouched, which is the
//! point: nothing below the boundary learned anything.

use pandemonium_sim_api::Event;

/// A placeholder audio cue: one named feedback moment. Six variants are
/// the Alpha's event-derived moments (combat, loss, production, economy,
/// and the match end); the three client-side moments (PLAN §4.2 — command
/// acknowledgment, selection, UI) have no sim event and are fed directly
/// via [`AudioSink::on_cue`], never through [`cue_for`].
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
    /// An order was submitted to the host (PLAN §4.2's command
    /// acknowledgment — a client-side moment: the sim's acceptance is a
    /// separate, later fact, surfaced through the refusal feedback).
    CommandAck,
    /// The selection's membership changed (a client-side moment).
    SelectionClick,
    /// A menu row was activated (a client-side moment).
    UiClick,
}

impl AudioCue {
    /// Every cue, in declaration order — the client's cue bank indexes its
    /// synthesized buffers with [`AudioCue::index`], so this order is the
    /// bank's layout. Pinned by test against the variant list.
    pub const ALL: [AudioCue; 9] = [
        AudioCue::AttackLanded,
        AudioCue::UnitLost,
        AudioCue::UnitReady,
        AudioCue::StructureDone,
        AudioCue::Delivery,
        AudioCue::MatchEnded,
        AudioCue::CommandAck,
        AudioCue::SelectionClick,
        AudioCue::UiClick,
    ];

    /// The cue's index into [`AudioCue::ALL`] (the cue bank's layout).
    pub fn index(self) -> usize {
        match self {
            AudioCue::AttackLanded => 0,
            AudioCue::UnitLost => 1,
            AudioCue::UnitReady => 2,
            AudioCue::StructureDone => 3,
            AudioCue::Delivery => 4,
            AudioCue::MatchEnded => 5,
            AudioCue::CommandAck => 6,
            AudioCue::SelectionClick => 7,
            AudioCue::UiClick => 8,
        }
    }

    /// The cue's log name (evidence lines name what they counted).
    pub fn name(self) -> &'static str {
        match self {
            AudioCue::AttackLanded => "AttackLanded",
            AudioCue::UnitLost => "UnitLost",
            AudioCue::UnitReady => "UnitReady",
            AudioCue::StructureDone => "StructureDone",
            AudioCue::Delivery => "Delivery",
            AudioCue::MatchEnded => "MatchEnded",
            AudioCue::CommandAck => "CommandAck",
            AudioCue::SelectionClick => "SelectionClick",
            AudioCue::UiClick => "UiClick",
        }
    }
}

/// The pure event → cue mapping (plan §11.5's "fed by events"). Every cue a
/// real sink would voice, derived from exactly the events the boundary
/// already emits — no new event variants, no sim changes. The three
/// client-side cues (CommandAck, SelectionClick, UiClick) deliberately have
/// no arm here: they have no sim event, which is their defining property.
/// Events with no audio moment (spawn bookkeeping, rejections, path
/// failures, depletion) map to `None`.
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
    /// One client-side cue with no sim event (PLAN §4.2: the command
    /// acknowledgment, selection click, and UI click moments). Called by
    /// the wiring layer at the moment itself — the same outward-only flow
    /// as `on_events`, minus the event.
    fn on_cue(&mut self, cue: AudioCue);
}

/// The headless sink: accepts and discards events. Exists so tests, soak
/// runs, and CI exercise exactly the path the windowed client feeds (the
/// same shape as the null renderer, plan §11.2). The M10.2 Phase 4 fallback
/// arm: a client with no audio device counts through this exact type
/// ("fall back to NullAudioSink"), so its counters are the wiring oracle.
#[derive(Default)]
pub struct NullAudioSink {
    /// How many events it has been fed (the smoke summary's evidence that
    /// the wiring ran — a counter, not audio).
    pub fed: usize,
    /// How many of those mapped to cues.
    pub cues: usize,
    /// How many client-side cues reached [`AudioSink::on_cue`] (PLAN §4.2:
    /// the direct path's evidence — the wiring tests' oracle).
    pub client_cues: usize,
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

    fn on_cue(&mut self, _cue: AudioCue) {
        self.client_cues += 1;
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

    #[test]
    fn the_null_sink_counts_client_side_cues_on_the_direct_path() {
        // PLAN §4.2's direct path: the three client-side moments reach
        // `on_cue` and advance their own counter — this is the wiring
        // oracle the client's fallback arm counts through.
        let mut sink = NullAudioSink::new();
        assert_eq!(sink.client_cues, 0);
        sink.on_cue(AudioCue::CommandAck);
        sink.on_cue(AudioCue::SelectionClick);
        sink.on_cue(AudioCue::UiClick);
        assert_eq!(sink.client_cues, 3);
        // The direct path is separate from the event path's counters.
        assert_eq!((sink.fed, sink.cues), (0, 0));
    }

    #[test]
    fn every_variant_is_in_all_once_with_a_dense_index_and_a_name() {
        // The cue bank's layout contract: ALL lists every variant exactly
        // once (9 of them), `index` is the dense 0..9 map into ALL, and
        // every variant names itself (evidence lines name what they
        // counted).
        assert_eq!(AudioCue::ALL.len(), 9);
        for (position, cue) in AudioCue::ALL.iter().enumerate() {
            assert_eq!(
                cue.index(),
                position,
                "{} sits at its ALL position",
                cue.name()
            );
            assert!(!cue.name().is_empty());
        }
        // Dense and injective: the indices are exactly 0..9.
        let mut indices: Vec<usize> = AudioCue::ALL.iter().map(|c| c.index()).collect();
        indices.sort_unstable();
        indices.dedup();
        assert_eq!(indices, (0..AudioCue::ALL.len()).collect::<Vec<usize>>());
        // The three client-side cues exist and are distinct variants.
        assert_eq!(AudioCue::CommandAck.name(), "CommandAck");
        assert_eq!(AudioCue::SelectionClick.name(), "SelectionClick");
        assert_eq!(AudioCue::UiClick.name(), "UiClick");
    }

    #[test]
    fn no_event_maps_to_a_client_side_cue() {
        // The defining property of the three client-side cues: no sim event
        // produces them, so `cue_for` has no arm that could — they arrive
        // only through the direct path.
        let every_event_shape = [
            hit(),
            died(),
            delivered(),
            rejected(),
            match_ended(),
            Event::ProductionCompleted {
                producer: EntityId(9),
                producible: pandemonium_sim_api::KindId(5),
            },
            Event::ConstructionCompleted { site: EntityId(28) },
        ];
        let client_side = [
            AudioCue::CommandAck,
            AudioCue::SelectionClick,
            AudioCue::UiClick,
        ];
        for event in &every_event_shape {
            let cue = cue_for(event);
            assert!(
                cue.is_none() || !client_side.contains(&cue.unwrap()),
                "no event may map to a client-side cue"
            );
        }
    }
}
