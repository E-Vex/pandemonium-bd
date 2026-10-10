//! The hardware-test gate (A-002 Part 1(a)): the sanctioned pattern for any
//! test that needs REAL GPU, window, or audio hardware.
//!
//! The rule: a hardware test runs only on a machine whose hardware is
//! POSITIVELY known to be plausible — never on a maybe. A headless CI runner,
//! a GPU-less Windows VM, or an audio-less mac must never reach the FFI call
//! to find out what happens; it skips first, visibly (the skip prints under
//! `cargo test -- --nocapture`, which is how the CI legs and the A-002
//! diagnostics workflow run the suite).
//!
//! `PANDEMONIUM_HW_TESTS=1` forces the gate open (the deliberate lever for
//! the Windows-crash isolation: run the suite on a runner with the real
//! wgpu/rodio calls turned ON and watch which test the access violation
//! lands in); `PANDEMONIUM_HW_TESTS=0` forces it closed (a dev machine can
//! opt out of hardware canaries without touching the tests).
//!
//! Positively-known plausibility, per scope, when the variable is unset:
//! - gpu: Linux with a display session (`DISPLAY`/`WAYLAND_DISPLAY`) AND
//!   `/dev/dri` — a desktop that can actually hand wgpu a surface. Xvfb
//!   without DRI nodes is NOT gpu-plausible (llvmpipe is not a promise).
//! - audio: Linux with `/dev/snd` (mirrors `sound::device_plausible`).
//! - Other operating systems have no cheap, crash-proof positive check, so
//!   the gate stays closed until the env lever says otherwise — the honest
//!   default on a machine we cannot interrogate without risking the very
//!   crash we are gating against.
//!
//! This module is test-only: product code decides its own degradation at
//! runtime (`Sound::new`'s probe fallback, `WgpuRenderer::new`'s error
//! return, the event-loop fallback to the headless smoke) — never through
//! this gate.

/// The hardware scopes a test can require.
#[cfg(test)]
pub enum Scope {
    /// A GPU adapter that wgpu can enumerate (and, in the product, a surface).
    Gpu,
    /// An audio output device the rodio probe can open.
    Audio,
}

/// Whether hardware tests for `scope` may run on this machine (see the
/// module doc for the exact law).
#[cfg(test)]
pub fn enabled(scope: Scope) -> bool {
    match std::env::var("PANDEMONIUM_HW_TESTS") {
        Ok(value) if value == "1" => return true,
        Ok(value) if value == "0" => return false,
        _ => {}
    }
    if !cfg!(target_os = "linux") {
        // No crash-free positive check on this platform — closed by default.
        return false;
    }
    match scope {
        Scope::Gpu => {
            let session = std::env::var_os("DISPLAY").is_some()
                || std::env::var_os("WAYLAND_DISPLAY").is_some();
            session && std::path::Path::new("/dev/dri").exists()
        }
        Scope::Audio => std::path::Path::new("/dev/snd").exists(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The gate itself is decidable and total: it answers for every scope
    /// without panicking, whatever the machine (the skip path is the product
    /// here — this test PINS that the gate can be asked cheaply).
    #[test]
    fn the_gate_answers_for_every_scope() {
        let gpu = enabled(Scope::Gpu);
        let audio = enabled(Scope::Audio);
        // On this machine the answers are facts, not expectations — print
        // them so `--nocapture` runs (the CI legs) show the gate state.
        println!("hw gate: gpu={gpu} audio={audio}");
    }
}
