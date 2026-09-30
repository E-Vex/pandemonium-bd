//! The render abstraction (plan §11.2): a `Renderer` trait boundary that keeps
//! the sim-facing engine code independent of wgpu, so a headless null renderer
//! exists for tests and soak runs. The wgpu implementation lives in `client`
//! (plan §3.2: wgpu is a client-only dependency).

use glam::{Mat4, Vec3};

use crate::interpolate::RenderSnapshot;
use pandemonium_sim_api::{ResourceId, Tick};

/// Presentation-only HUD and debug data for one frame: the §11.4 resource and
/// population display plus the §11.6 debug overlays (tick counter, state hash,
/// pause state). Values cross the sim boundary as plain data — gathered by the
/// client from its [`crate::MatchHost`], never read from simulation internals.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct HudState {
    /// The tick the next step will apply (the §11.6 tick counter).
    pub tick: Tick,
    /// The canonical state hash of the current tick (§11.6).
    pub state_hash: u64,
    /// Whether the host is paused (§11.6 pause / single-step debugging).
    pub paused: bool,
    /// The viewed player's resource ledger, ascending by resource id.
    pub resources: Vec<(ResourceId, i64)>,
    /// The viewed player's population usage (economy systems arrive in M5).
    pub population: u32,
    /// The viewed player's population cap (structures provide it from M5).
    pub population_cap: u32,
}

/// Everything one draw call needs: the interpolated entities, the camera's
/// view-projection, the client's current selection (for highlights —
/// selection state lives in the client, plan §11.3), and the HUD/debug data
/// for the overlay pass (plan §11.4, §11.6). Presentation-only data — never
/// simulation state.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    /// The blended snapshot to draw.
    pub snapshot: &'a RenderSnapshot,
    /// The camera's combined view-projection (world -> clip).
    pub view_projection: Mat4,
    /// The camera's world position (for view-dependent effects).
    pub eye: Vec3,
    /// The selected entity ids (client-local state, echoed for rendering).
    pub selection: &'a [pandemonium_sim_api::EntityId],
    /// HUD and debug overlay data (resources/population, tick, state hash,
    /// pause state) for the UI pass.
    pub hud: &'a HudState,
}

/// A rendering backend. Implementations draw [`Frame`]s; they never touch the
/// simulation (FD-6, FD-9 — they receive interpolated snapshots only).
pub trait Renderer {
    /// Draws one frame.
    fn render(&mut self, frame: Frame<'_>);
}

/// The headless renderer: accepts and discards frames, counting them. It
/// exists so tests, soak runs, and CI machines without a GPU exercise exactly
/// the same engine path as the windowed client (plan §11.2).
#[derive(Clone, Copy, Debug, Default)]
pub struct NullRenderer {
    frames: u64,
}

impl NullRenderer {
    /// A fresh null renderer.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many frames have been rendered (the observable behavior of the
    /// null renderer — tests assert on it).
    pub fn frames_rendered(&self) -> u64 {
        self.frames
    }
}

impl Renderer for NullRenderer {
    fn render(&mut self, _frame: Frame<'_>) {
        self.frames += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interpolate::Interpolator;

    #[test]
    fn the_null_renderer_counts_frames_and_accepts_any_camera() {
        let mut renderer = NullRenderer::new();
        let snapshot = Interpolator::new().render(0.5);
        let hud = HudState::default();
        for _ in 0..3 {
            renderer.render(Frame {
                snapshot: &snapshot,
                view_projection: Mat4::IDENTITY,
                eye: Vec3::ZERO,
                selection: &[],
                hud: &hud,
            });
        }
        assert_eq!(renderer.frames_rendered(), 3);
    }

    #[test]
    fn hud_state_is_plain_data_with_documented_fields() {
        let hud = HudState {
            tick: 12,
            state_hash: 0xABCD,
            paused: true,
            resources: vec![(ResourceId(0), 200)],
            population: 4,
            population_cap: 10,
        };
        assert_eq!(hud.tick, 12);
        assert_eq!(hud.resources.len(), 1);
        // Equality over every field keeps accidental field drops review-visible.
        assert_eq!(hud, hud.clone());
        assert_ne!(hud, HudState::default());
    }
}
