//! The render abstraction (plan §11.2): a `Renderer` trait boundary that keeps
//! the sim-facing engine code independent of wgpu, so a headless null renderer
//! exists for tests and soak runs. The wgpu implementation lives in `client`
//! (plan §3.2: wgpu is a client-only dependency).

use glam::{Mat4, Vec3};

use crate::interpolate::RenderSnapshot;

/// Everything one draw call needs: the interpolated entities, the camera's
/// view-projection, and the client's current selection (for highlights —
/// selection state lives in the client, plan §11.3). Presentation-only
/// data — never simulation state.
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
        for _ in 0..3 {
            renderer.render(Frame {
                snapshot: &snapshot,
                view_projection: Mat4::IDENTITY,
                eye: Vec3::ZERO,
                selection: &[],
            });
        }
        assert_eq!(renderer.frames_rendered(), 3);
    }
}
