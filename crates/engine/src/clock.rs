//! The fixed-timestep accumulator (plan §11.1): real-time deltas accumulate, the
//! simulation steps at exactly [`pandemonium_sim::TICKS_PER_SECOND`], and
//! rendering runs at display rate, interpolating between the previous and
//! current snapshots by [`FixedTimestep::alpha`].
//!
//! Time originates here and in the client — never inside the simulation (FD-6).
//! Catch-up is capped at [`MAX_CATCH_UP_STEPS`] ticks per frame so a long stall
//! cannot cause a spiral of death; the backlog beyond the cap is dropped (the
//! alternative — falling ever further behind — is the spiral itself).

use std::time::Duration;

/// The catch-up cap per `update` call (plan §11.1: "cap catch-up at e.g. 5
/// ticks per frame to avoid spiral of death").
pub const MAX_CATCH_UP_STEPS: u32 = 5;

/// The accumulator loop. Feed real elapsed time into [`FixedTimestep::update`],
/// run the returned number of simulation steps, then read
/// [`FixedTimestep::alpha`] to interpolate rendering.
#[derive(Clone, Copy, Debug)]
pub struct FixedTimestep {
    tick_duration: Duration,
    accumulator: Duration,
    max_catch_up_steps: u32,
}

impl FixedTimestep {
    /// A loop stepping at `ticks_per_second` (the match passes
    /// [`pandemonium_sim::TICKS_PER_SECOND`]).
    pub fn new(ticks_per_second: u32) -> Self {
        assert!(ticks_per_second > 0, "the tick rate must be positive");
        Self {
            tick_duration: Duration::from_secs(1) / ticks_per_second,
            accumulator: Duration::ZERO,
            max_catch_up_steps: MAX_CATCH_UP_STEPS,
        }
    }

    /// The simulation's tick duration this loop steps at.
    pub fn tick_duration(&self) -> Duration {
        self.tick_duration
    }

    /// Feeds real elapsed time and returns how many simulation steps are due
    /// now (0..=catch-up cap). Backlog beyond the cap is dropped.
    pub fn update(&mut self, real_dt: Duration) -> u32 {
        self.accumulator = self.accumulator.saturating_add(real_dt);
        let mut steps = 0u32;
        while steps < self.max_catch_up_steps && self.accumulator >= self.tick_duration {
            self.accumulator -= self.tick_duration;
            steps += 1;
        }
        if steps == self.max_catch_up_steps && self.accumulator >= self.tick_duration {
            // A stall longer than the whole catch-up budget: drop the backlog
            // and continue from now (the spiral-of-death guard).
            self.accumulator = Duration::ZERO;
        }
        steps
    }

    /// The interpolation fraction toward the next tick: `accumulator /
    /// tick_duration`, in `0.0..1.0`.
    pub fn alpha(&self) -> f32 {
        (self.accumulator.as_secs_f64() / self.tick_duration.as_secs_f64()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT_60HZ: Duration = Duration::from_millis(16);

    #[test]
    fn one_second_of_sixty_hz_frames_is_thirty_steps() {
        let mut clock = FixedTimestep::new(pandemonium_sim::TICKS_PER_SECOND);
        let frame = Duration::from_secs_f64(1.0 / 60.0); // exactly 60 Hz
        let mut steps = 0u32;
        for _ in 0..60 {
            steps += clock.update(frame);
        }
        assert_eq!(steps, 30, "30 ticks per second at a 60 Hz feed");
        assert_eq!(
            clock.tick_duration(),
            Duration::from_nanos(1_000_000_000 / 30)
        );
    }

    #[test]
    fn alpha_stays_in_unit_range() {
        let mut clock = FixedTimestep::new(pandemonium_sim::TICKS_PER_SECOND);
        for _ in 0..100 {
            let _ = clock.update(Duration::from_millis(7));
            assert!((0.0..1.0).contains(&clock.alpha()), "alpha out of range");
        }
    }

    #[test]
    fn a_long_stall_is_capped_and_the_backlog_dropped() {
        let mut clock = FixedTimestep::new(pandemonium_sim::TICKS_PER_SECOND);
        // Ten seconds in one frame: 300 due ticks, capped at 5, then reset.
        let steps = clock.update(Duration::from_secs(10));
        assert_eq!(steps, MAX_CATCH_UP_STEPS);
        // The backlog is gone: the next 16 ms frame owes zero steps.
        assert_eq!(clock.update(DT_60HZ), 0);
    }

    #[test]
    fn partial_frames_accumulate() {
        let mut clock = FixedTimestep::new(pandemonium_sim::TICKS_PER_SECOND);
        let tick_ms = 1000.0 / pandemonium_sim::TICKS_PER_SECOND as f64;
        // Feed a bit less than a tick at a time: two frames make one step.
        let almost = Duration::from_secs_f64(tick_ms / 1000.0 * 0.6);
        assert_eq!(clock.update(almost), 0);
        assert_eq!(clock.update(almost), 1);
    }
}
