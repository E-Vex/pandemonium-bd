//! The world-input key map (PLAN-M11 §4 phase 1): ONE pure table that
//! translates a physical key press (plus the Ctrl modifier) into the
//! action the in-match dispatch takes — the same table the `main.rs`
//! handler and the unit tests read, so the two cannot drift apart.
//!
//! Phase 1 is the Zero Hour "free the keyboard" pass: W/A/S/D/Q/E lose
//! their camera duties (arrows are the keyboard scroll, as in ZH; A is
//! only the attack-move arm, S only Stop, W reserved for air, D free),
//! and the two tools move off the F1–F8 row the camera bookmarks will
//! own (debug overlay F3 → F10, bug report F8 → F11).
//!
//! Everything here is pure `(KeyCode, modifiers, pressed) -> action`.
//! The dispatch layer owns all state (held keys, clocks, the camera);
//! `CameraHold` is the only action that also fires on release, because
//! a held-scroll key must leave the held set whatever modifier state
//! the release carries (a stuck "up" would drift the camera forever).

use winit::keyboard::KeyCode;

/// A camera-scroll direction a held key contributes (the held-key set
/// stores these as their tag names; `about_to_wait` reads them back).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraDir {
    /// Toward the top of the screen.
    Up,
    /// Toward the bottom of the screen.
    Down,
    /// Toward the left edge of the screen.
    Left,
    /// Toward the right edge of the screen.
    Right,
}

impl CameraDir {
    /// The held-key set's tag for this direction (`set_key` stores it).
    pub fn tag(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
        }
    }
}

/// What one in-match key event means (PLAN-M11 §2's tables, as they
/// stand after each phase lands; rows still unbound are `Unbound`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorldKeyAction {
    /// A camera-scroll key: hold-to-scroll. Fires on press AND release —
    /// the release must always clear the held entry (see the module doc).
    CameraHold { dir: CameraDir, pressed: bool },
    /// A — arm attack-move (the next left-click places it).
    ArmAttackMove,
    /// S — stop the selection.
    Stop,
    /// F10 — toggle the §11.6 debug overlay.
    ToggleDebugOverlay,
    /// F11 — write the deterministic bug-report dump.
    BugReport,
    /// P — pause / unpause.
    Pause,
    /// `.` — single-step while paused.
    SingleStep,
    /// A digit — control group duties (assign with Ctrl, recall,
    /// double-tap centers). `index` is 0-based into the group array.
    ControlGroup { index: usize },
    /// R — rematch, once the match has ended.
    Rematch,
    /// Escape — the cancellation ladder.
    Escape,
    /// Space — jump to the last radar event (else the base).
    JumpLastEvent,
    /// The key is not bound in the world map (menus may still bind it).
    Unbound,
}

/// Resolves one in-match key event to its action. `_ctrl` is the Ctrl
/// modifier state at the event; phase 1 has no Ctrl-sensitive rows yet
/// (the cycling and bookmark-set rows arrive in phases 2 and 4), but the
/// parameter is part of the table's fixed signature so the dispatch
/// never changes shape between phases.
pub fn world_key_action(code: KeyCode, _ctrl: bool, pressed: bool) -> WorldKeyAction {
    // The arrows are the keyboard scroll (ZH). With Ctrl held a press
    // belongs to the cycling rows instead (phase 2); the release still
    // clears the held entry so the scroll never sticks.
    let camera = |dir: CameraDir| WorldKeyAction::CameraHold { dir, pressed };
    match code {
        KeyCode::ArrowUp => camera(CameraDir::Up),
        KeyCode::ArrowDown => camera(CameraDir::Down),
        KeyCode::ArrowLeft => camera(CameraDir::Left),
        KeyCode::ArrowRight => camera(CameraDir::Right),
        // A arms attack-move only (its camera duty is gone — ZH).
        KeyCode::KeyA if pressed => WorldKeyAction::ArmAttackMove,
        // S stops only.
        KeyCode::KeyS if pressed => WorldKeyAction::Stop,
        KeyCode::KeyP if pressed => WorldKeyAction::Pause,
        KeyCode::Period if pressed => WorldKeyAction::SingleStep,
        // The tools, relocated off the F1–F8 row (phase 1): F10 debug,
        // F11 bug report. F3 and F8 end up unbound.
        KeyCode::F10 if pressed => WorldKeyAction::ToggleDebugOverlay,
        KeyCode::F11 if pressed => WorldKeyAction::BugReport,
        KeyCode::KeyR if pressed => WorldKeyAction::Rematch,
        KeyCode::Escape if pressed => WorldKeyAction::Escape,
        KeyCode::Space if pressed => WorldKeyAction::JumpLastEvent,
        // Control groups 1–9 (Digit0 joins them in phase 2). The Ctrl
        // modifier decides assign-vs-recall inside the dispatch.
        KeyCode::Digit1 if pressed => WorldKeyAction::ControlGroup { index: 0 },
        KeyCode::Digit2 if pressed => WorldKeyAction::ControlGroup { index: 1 },
        KeyCode::Digit3 if pressed => WorldKeyAction::ControlGroup { index: 2 },
        KeyCode::Digit4 if pressed => WorldKeyAction::ControlGroup { index: 3 },
        KeyCode::Digit5 if pressed => WorldKeyAction::ControlGroup { index: 4 },
        KeyCode::Digit6 if pressed => WorldKeyAction::ControlGroup { index: 5 },
        KeyCode::Digit7 if pressed => WorldKeyAction::ControlGroup { index: 6 },
        KeyCode::Digit8 if pressed => WorldKeyAction::ControlGroup { index: 7 },
        KeyCode::Digit9 if pressed => WorldKeyAction::ControlGroup { index: 8 },
        // W is reserved for air selection (no air content yet) and D is
        // simply free; Q and E await their selection duties (phase 2).
        // F3 and F8 are vacated for the bookmark row.
        KeyCode::KeyW
        | KeyCode::KeyD
        | KeyCode::KeyQ
        | KeyCode::KeyE
        | KeyCode::F3
        | KeyCode::F8 => WorldKeyAction::Unbound,
        _ => WorldKeyAction::Unbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the phase 1 key map (PLAN-M11 §4 phase 1's acceptance) ---------

    #[test]
    fn the_arrows_are_the_keyboard_scroll() {
        for (code, dir) in [
            (KeyCode::ArrowUp, CameraDir::Up),
            (KeyCode::ArrowDown, CameraDir::Down),
            (KeyCode::ArrowLeft, CameraDir::Left),
            (KeyCode::ArrowRight, CameraDir::Right),
        ] {
            assert_eq!(
                world_key_action(code, false, true),
                WorldKeyAction::CameraHold { dir, pressed: true },
                "{code:?} scrolls on press"
            );
            // The release clears the held entry whatever Ctrl reads at
            // that moment (a stuck direction would drift the camera).
            assert_eq!(
                world_key_action(code, true, false),
                WorldKeyAction::CameraHold {
                    dir,
                    pressed: false
                },
                "{code:?} release always clears the held entry"
            );
        }
    }

    #[test]
    fn wasd_and_qe_are_not_camera_keys() {
        // ZH: W/A/S/D/Q/E are action keys, not camera keys. A arms
        // attack-move; S stops; the rest are unbound in the world map.
        for code in [KeyCode::KeyW, KeyCode::KeyD, KeyCode::KeyQ, KeyCode::KeyE] {
            assert_eq!(
                world_key_action(code, false, true),
                WorldKeyAction::Unbound,
                "{code:?} has no camera duty"
            );
        }
    }

    #[test]
    fn a_is_only_the_attack_move_arm_and_s_is_only_stop() {
        assert_eq!(
            world_key_action(KeyCode::KeyA, false, true),
            WorldKeyAction::ArmAttackMove
        );
        assert_eq!(
            world_key_action(KeyCode::KeyS, false, true),
            WorldKeyAction::Stop
        );
        // Key releases of action keys are inert (the CameraHold arms
        // above are the only release-sensitive rows).
        assert_eq!(
            world_key_action(KeyCode::KeyA, false, false),
            WorldKeyAction::Unbound
        );
        assert_eq!(
            world_key_action(KeyCode::KeyS, false, false),
            WorldKeyAction::Unbound
        );
    }

    #[test]
    fn the_tools_live_on_f10_and_f11() {
        assert_eq!(
            world_key_action(KeyCode::F10, false, true),
            WorldKeyAction::ToggleDebugOverlay
        );
        assert_eq!(
            world_key_action(KeyCode::F11, false, true),
            WorldKeyAction::BugReport
        );
        // The F1–F8 row is vacated for the camera bookmarks (phase 4
        // binds it); F3 and F8 do nothing today.
        for code in [KeyCode::F3, KeyCode::F8] {
            assert_eq!(world_key_action(code, false, true), WorldKeyAction::Unbound);
        }
    }

    #[test]
    fn the_stable_rows_keep_their_actions() {
        assert_eq!(
            world_key_action(KeyCode::KeyP, false, true),
            WorldKeyAction::Pause
        );
        assert_eq!(
            world_key_action(KeyCode::Period, false, true),
            WorldKeyAction::SingleStep
        );
        assert_eq!(
            world_key_action(KeyCode::KeyR, false, true),
            WorldKeyAction::Rematch
        );
        assert_eq!(
            world_key_action(KeyCode::Escape, false, true),
            WorldKeyAction::Escape
        );
        assert_eq!(
            world_key_action(KeyCode::Space, false, true),
            WorldKeyAction::JumpLastEvent
        );
    }

    #[test]
    fn digits_map_to_their_control_groups() {
        for (digit, index) in [
            (KeyCode::Digit1, 0),
            (KeyCode::Digit5, 4),
            (KeyCode::Digit9, 8),
        ] {
            assert_eq!(
                world_key_action(digit, false, true),
                WorldKeyAction::ControlGroup { index },
                "{digit:?} is group index {index}"
            );
        }
    }
}
