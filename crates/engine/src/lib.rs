//! Engine layer (plan §11): the fixed-timestep game loop, snapshot interpolation,
//! input-to-command mapping, camera, UI toolkit, and render abstraction.
//!
//! The engine steps the simulation at exactly 30 Hz and renders at display rate,
//! interpolating between the previous and current snapshots by the accumulator
//! fraction. Time originates here and in the client — never inside the simulation
//! (FD-6). A `Renderer` trait keeps a headless null renderer available for tests and
//! soak runs. Window and GPU code live in `client` (plan §3.2).
//!
//! This crate lands with milestone M3.

#![forbid(unsafe_code)]
