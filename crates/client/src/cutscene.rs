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

#[cfg(test)]
mod tests {
    use super::*;

    /// The Phase 1 scene over pinned anchors (the crossroads start corners'
    /// shape: enemy start bottom-right, player start top-left).
    fn scene() -> Cutscene {
        Cutscene::mission_start(
            "Crossroads",
            "Objectives: Destroy all enemy structures; Train 4 Worker (optional)",
            (56.0, 56.0),
            (8.5, 8.5),
        )
    }

    #[test]
    fn the_scene_eases_from_the_enemy_start_to_the_player_start() {
        // t=0: the camera sits on the enemy's start; the distance is the
        // opening view's. The midpoint eases to the average (smoothstep's
        // symmetric middle); t=1: the player's start.
        let mut scene = scene();
        let (x, z, distance) = scene.camera_focus();
        assert_eq!((x, z), (56.0, 56.0));
        assert_eq!(distance, crate::START_CAMERA_DISTANCE);
        scene.advance(MISSION_START_SECONDS * 0.5);
        let (x, z, _) = scene.camera_focus();
        assert_eq!((x, z), (32.25, 32.25), "the eased midpoint");
        for _ in 0..60 {
            scene.advance(MISSION_START_SECONDS / 60.0);
        }
        let focus = scene.camera_focus();
        assert_eq!(
            (focus.0, focus.1),
            (8.5, 8.5),
            "the pan lands on the player's start"
        );
        assert_eq!(focus, scene.resting_focus());
    }

    #[test]
    fn advancing_past_the_duration_finishes_the_scene() {
        // A-011 Task 5: the scene ends when it runs its duration — the
        // wiring layer then unpauses (the same path a skip takes).
        let mut scene = scene();
        assert_eq!(scene.advance(1.0), SceneProgress::Playing);
        assert!(!scene.is_over());
        assert_eq!(
            scene.advance(MISSION_START_SECONDS),
            SceneProgress::Finished
        );
        assert!(scene.is_over());
        // Further advances stay finished (idempotent at the duration cap).
        assert_eq!(scene.advance(0.5), SceneProgress::Finished);
    }

    #[test]
    fn skip_input_ends_the_scene_at_the_resting_focus() {
        // A-011 Task 5: the skip edge — any moment it lands on, the scene is
        // over and the camera reads the resting focus (the player's start,
        // the readable opening view). The app-level unpause is pinned in
        // main.rs's mission wiring tests.
        let mut scene = scene();
        scene.advance(0.7); // somewhere mid-pan
        assert!(!scene.is_over());
        let mid_focus = scene.camera_focus();
        assert_ne!(mid_focus, scene.resting_focus());
        scene.skip();
        assert!(scene.is_over());
        assert_eq!(scene.camera_focus(), scene.resting_focus());
    }

    #[test]
    fn the_captions_carry_the_mission_name_and_intro_in_ascii() {
        // The caption is the mission's display name plus the one-line intro
        // (the objective summary); every string is ASCII (the embedded font
        // subset, ADR-0201 §5).
        let scene = scene();
        let (title, line) = scene.captions();
        assert_eq!(title, "Crossroads");
        assert!(line.starts_with("Objectives: "));
        assert!(title.is_ascii() && line.is_ascii());
        // The caption panel draws quads inside the viewport.
        let atlas = crate::text::TextAtlas::new();
        let quads = scene.caption_quads(&atlas, (1280.0, 800.0));
        assert!(!quads.is_empty());
        for quad in &quads {
            assert!(quad.x >= 0.0 && quad.x + quad.w <= 1280.0 + 0.5);
            assert!(quad.y >= 0.0 && quad.y + quad.h <= 800.0 + 0.5);
        }
    }
}
