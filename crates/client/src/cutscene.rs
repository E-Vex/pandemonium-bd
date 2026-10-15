//! The live-state cutscene player (ADR-0201 §5, option (i) — Phase 1).
//!
//! A cutscene runs while the host is paused: pause freezes the clock and
//! discards elapsed time (`host.rs`'s `advance` early-return), so no tick
//! passes, no command applies, and the pause flag itself is host state, not
//! sim state. The scene moves the camera over the frozen live world and
//! shows ASCII captions through the existing text pipeline (the embedded
//! font is an ASCII subset, ADR-0201 §5). It never submits commands, never
//! reads sim internals, and consumes only views, events, and content-adjacent
//! presentation data (FD-9) — the same discipline as the audio and feedback
//! layers. The state hash and every golden are provably out of reach.
//!
//! Skipping is an input event (ADR-0201 §5 (iii): every cutscene is
//! skippable at any moment): any key — Esc included — ends the scene and
//! unpauses; the wiring layer ([`crate::app::App::cutscene_input`]) applies
//! that to the host. Commands submitted after resume are stamped with the
//! sim's current tick at submit, so the command log's semantics are untouched
//! by the interruption.
//!
//! Phase 1 ships one scene: the mission-start pan (enemy start -> player
//! start) with the mission's display name and a one-line objective summary.
//! The module is the reusable player — later scenes (objective hooks,
//! Phase 3) construct more [`Cutscene`]s through the same seams.

use crate::text::{TextAtlas, UiQuad};

/// The mission-start scene's duration in seconds — a "brief camera pan over
/// the map" (ADR-0201 §9): long enough to read the caption, short enough
/// that skipping never feels required.
pub const MISSION_START_SECONDS: f32 = 6.0;

/// The skip hint drawn under the caption (ASCII).
const SKIP_HINT: &str = "press any key to skip";

/// Whether a scene still runs after the last advance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneProgress {
    /// The scene continues (the camera keeps easing).
    Playing,
    /// The scene ran its duration — end it and unpause (the wiring layer's
    /// cue, identical to the skip path).
    Finished,
}

/// One live-state cutscene: the camera eases between two ground anchors at a
/// fixed distance while a two-line ASCII caption rides over the frozen live
/// world. Pure data plus the easing math — no winit types, no host, no sim
/// (the wiring layer drives the camera from [`Cutscene::camera_focus`] and
/// renders [`Cutscene::caption_quads`] each frame).
#[derive(Clone, Debug)]
pub struct Cutscene {
    /// The caption's title line (the mission's display name).
    title: String,
    /// The caption's body line (the one-line intro).
    line: String,
    /// Where the camera starts (the enemy's start anchor).
    from: (f32, f32),
    /// Where the camera rests (the player's start anchor — the readable
    /// opening view the match resumes on).
    to: (f32, f32),
    /// The camera's orbit distance for the whole pan (the opening distance).
    distance: f32,
    /// The scene's duration in seconds.
    duration_s: f32,
    /// How much of it has elapsed.
    elapsed_s: f32,
}

impl Cutscene {
    /// The Phase 1 scene (ADR-0201 §9): a brief pan over the map from the
    /// enemy's start to the player's start, captioned with the mission's
    /// display name and a one-line intro (the objective summary). Both
    /// anchors and the distance come from the map's declared starts and the
    /// opening view — content-adjacent presentation data, never sim state.
    pub fn mission_start(
        display_name: &str,
        intro_line: &str,
        from: (f32, f32),
        to: (f32, f32),
    ) -> Self {
        Self {
            title: display_name.to_string(),
            line: intro_line.to_string(),
            from,
            to,
            distance: crate::START_CAMERA_DISTANCE,
            duration_s: MISSION_START_SECONDS,
            elapsed_s: 0.0,
        }
    }

    /// The scene's captions: the title line and the body line (both ASCII).
    pub fn captions(&self) -> (&str, &str) {
        (&self.title, &self.line)
    }

    /// The eased camera focus at the current progress: smoothstep between
    /// the anchors (slow start, slow landing — no mechanical linear sweep),
    /// the fixed opening distance throughout.
    pub fn camera_focus(&self) -> (f32, f32, f32) {
        let t = self.progress();
        let ease = t * t * (3.0 - 2.0 * t);
        let x = self.from.0 + (self.to.0 - self.from.0) * ease;
        let z = self.from.1 + (self.to.1 - self.from.1) * ease;
        (x, z, self.distance)
    }

    /// Where the camera lands when the scene ends (skip or duration): the
    /// player's start at the opening distance — the same readable view a
    /// mission-less match opens on, whatever moment the skip landed on.
    pub fn resting_focus(&self) -> (f32, f32, f32) {
        (self.to.0, self.to.1, self.distance)
    }

    /// The normalized progress through the scene (0..=1).
    fn progress(&self) -> f32 {
        if self.duration_s <= 0.0 {
            return 1.0;
        }
        (self.elapsed_s / self.duration_s).clamp(0.0, 1.0)
    }

    /// Whether the scene is over (it ran its duration or was skipped).
    pub fn is_over(&self) -> bool {
        self.elapsed_s >= self.duration_s
    }

    /// Advances the scene by one frame's delta. `Finished` means the scene
    /// ran its duration — the wiring layer ends it and unpauses (the same
    /// path a skip takes).
    pub fn advance(&mut self, dt: f32) -> SceneProgress {
        self.elapsed_s = (self.elapsed_s + dt.max(0.0)).min(self.duration_s);
        if self.is_over() {
            SceneProgress::Finished
        } else {
            SceneProgress::Playing
        }
    }

    /// Ends the scene early (the skip path — ADR-0201 §5 (iii)); the camera
    /// then reads from [`Cutscene::resting_focus`].
    pub fn skip(&mut self) {
        self.elapsed_s = self.duration_s;
    }

    /// The caption panel: a subtitle-style panel near the bottom of the
    /// viewport — the title, the intro line, and the skip hint, centered,
    /// ASCII-only text through the shared atlas (the existing text pipeline).
    pub fn caption_quads(&self, atlas: &TextAtlas, viewport: (f32, f32)) -> Vec<UiQuad> {
        /// Padding inside the panel.
        const PAD: f32 = 10.0;
        /// The panel's clearance from the viewport's bottom edge.
        const BOTTOM_CLEARANCE: f32 = 40.0;
        let line_height = atlas.line_height.max(atlas.ascent + atlas.descent);
        let (title, line) = self.captions();
        let lines = [
            (title, [1.0, 0.9, 0.55, 1.0]),
            (line, [1.0, 1.0, 1.0, 0.95]),
        ];
        let mut widest = atlas.measure(SKIP_HINT);
        for (text, _) in &lines {
            widest = widest.max(atlas.measure(text));
        }
        let panel_w = (widest + 2.0 * PAD).min(viewport.0 * 0.9);
        let panel_h = (lines.len() + 1) as f32 * line_height + 2.0 * PAD;
        let panel_x = viewport.0 * 0.5 - panel_w * 0.5;
        let panel_y = viewport.1 - panel_h - BOTTOM_CLEARANCE;
        let mut quads =
            vec![atlas.solid_rect(panel_x, panel_y, panel_w, panel_h, [0.0, 0.0, 0.0, 0.7])];
        for (index, (text, color)) in lines.iter().enumerate() {
            let text_w = atlas.measure(text);
            quads.extend(atlas.layout(
                text,
                panel_x + (panel_w - text_w) * 0.5,
                panel_y + PAD + atlas.ascent + index as f32 * line_height,
                *color,
            ));
        }
        let hint_w = atlas.measure(SKIP_HINT);
        quads.extend(atlas.layout(
            SKIP_HINT,
            panel_x + (panel_w - hint_w) * 0.5,
            panel_y + PAD + atlas.ascent + lines.len() as f32 * line_height,
            [0.6, 0.66, 0.74, 0.85],
        ));
        quads
    }
}
