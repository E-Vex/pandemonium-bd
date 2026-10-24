//! The world-input key map (PLAN-M11 §4 phase 1 + phase 2): ONE pure table
//! that translates a physical key press (plus the Ctrl/Alt modifiers) into
//! the action the in-match dispatch takes — the same table the `main.rs`
//! handler and the unit tests read, so the two cannot drift apart.
//!
//! Phase 1 freed the keyboard (W/A/S/D/Q/E lost their camera duties; the
//! tools moved to F10/F11). Phase 2 gives Q and E their Zero Hour
//! selection duties, extends the control groups to ten (Digit0), adds
//! Alt+digit group viewing and Ctrl+arrow unit/worker cycling, and H's
//! jump to the Command Center.
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

/// The modifier state a key event carries (the table's second input).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyMods {
    /// Ctrl held (group assign, unit/worker cycling).
    pub ctrl: bool,
    /// Alt held (group view, waypoints — the gesture key of phase 3).
    pub alt: bool,
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
    /// A digit without modifiers — recall the control group (a double
    /// tap also centers; the dispatch's clock decides).
    RecallGroup { index: usize },
    /// Ctrl + digit — assign the current selection to the group.
    AssignGroup { index: usize },
    /// Alt + digit — view the group (camera only; the selection is not
    /// touched).
    ViewGroup { index: usize },
    /// R — rematch, once the match has ended.
    Rematch,
    /// Escape — the cancellation ladder.
    Escape,
    /// Space — jump to the last radar event (else the base).
    JumpLastEvent,
    /// E — select same-kind units on screen (the dispatch's double-tap
    /// clock widens it to the whole visible map).
    SelectSameKind,
    /// Q — select all own combat units on screen (double-tap: the whole
    /// visible map).
    SelectCombat,
    /// G — arm guard mode (the next left-click moves there; on arrival
    /// the unit stands and engages in-range targets — ZH guard under this
    /// sim's laws).
    ArmGuard,
    /// X — scatter: one Move per selected unit along an 8-ray ring.
    Scatter,
    /// Ctrl + Left/Right — cycle the selection through own units in
    /// ascending-id order, wrapping.
    CycleUnit { forward: bool },
    /// Ctrl + Up/Down — the same cycle restricted to workers.
    CycleWorker { forward: bool },
    /// H — jump the camera to the own Command Center (base fallback).
    JumpToBase,
    /// The key is not bound in the world map (menus may still bind it).
    Unbound,
}

/// Resolves one in-match key event to its action (the ONE table — the
/// dispatch and the tests read this and nothing else).
pub fn world_key_action(code: KeyCode, mods: KeyMods, pressed: bool) -> WorldKeyAction {
    // The arrows are the keyboard scroll (ZH). With Ctrl held a press
    // belongs to the cycling rows instead; the release still clears the
    // held entry so the scroll never sticks (PLAN-M11 §4 phase 2: the
    // cycle keys "must not pan the camera").
    let camera = |dir: CameraDir| WorldKeyAction::CameraHold { dir, pressed };
    if mods.ctrl {
        match code {
            KeyCode::ArrowLeft if pressed => return WorldKeyAction::CycleUnit { forward: false },
            KeyCode::ArrowRight if pressed => return WorldKeyAction::CycleUnit { forward: true },
            KeyCode::ArrowUp if pressed => return WorldKeyAction::CycleWorker { forward: true },
            KeyCode::ArrowDown if pressed => return WorldKeyAction::CycleWorker { forward: false },
            _ => {}
        }
    }
    // Control groups: digits 1-0 are ten groups (PLAN-M11 §2; Digit0 is
    // group 10). Ctrl assigns, Alt views, plain recalls. When both are
    // held Ctrl wins (an assign is the rarer, more deliberate gesture —
    // recorded as the table's tie rule).
    let group = |index: usize| {
        if mods.ctrl {
            WorldKeyAction::AssignGroup { index }
        } else if mods.alt {
            WorldKeyAction::ViewGroup { index }
        } else {
            WorldKeyAction::RecallGroup { index }
        }
    };
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
        // PLAN-M11 §2 selection keys (phase 2). W stays reserved for air
        // combat units (no air content — inert, the card says so), and
        // Ctrl+H stays reserved for a hero (inert).
        KeyCode::KeyE if pressed && !mods.ctrl && !mods.alt => WorldKeyAction::SelectSameKind,
        KeyCode::KeyQ if pressed && !mods.ctrl && !mods.alt => WorldKeyAction::SelectCombat,
        // PLAN-M11 §2 phase 3: G arms guard, X scatters (both plain keys —
        // ZH binds them without modifiers).
        KeyCode::KeyG if pressed && !mods.ctrl && !mods.alt => WorldKeyAction::ArmGuard,
        KeyCode::KeyX if pressed && !mods.ctrl && !mods.alt => WorldKeyAction::Scatter,
        KeyCode::KeyH if pressed && !mods.ctrl => WorldKeyAction::JumpToBase,
        KeyCode::Digit1 if pressed => group(0),
        KeyCode::Digit2 if pressed => group(1),
        KeyCode::Digit3 if pressed => group(2),
        KeyCode::Digit4 if pressed => group(3),
        KeyCode::Digit5 if pressed => group(4),
        KeyCode::Digit6 if pressed => group(5),
        KeyCode::Digit7 if pressed => group(6),
        KeyCode::Digit8 if pressed => group(7),
        KeyCode::Digit9 if pressed => group(8),
        KeyCode::Digit0 if pressed => group(9),
        // W is reserved for air selection (no air content yet), D is
        // simply free, Ctrl+H is reserved for a hero. F3 and F8 are
        // vacated for the bookmark row.
        KeyCode::KeyW | KeyCode::KeyD | KeyCode::F3 | KeyCode::F8 => WorldKeyAction::Unbound,
        _ => WorldKeyAction::Unbound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: KeyMods = KeyMods {
        ctrl: false,
        alt: false,
    };
    const CTRL: KeyMods = KeyMods {
        ctrl: true,
        alt: false,
    };
    const ALT: KeyMods = KeyMods {
        ctrl: false,
        alt: true,
    };

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
                world_key_action(code, PLAIN, true),
                WorldKeyAction::CameraHold { dir, pressed: true },
                "{code:?} scrolls on press"
            );
            // The release clears the held entry whatever the modifiers
            // read at that moment (a stuck direction would drift the
            // camera).
            assert_eq!(
                world_key_action(code, CTRL, false),
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
        // attack-move; S stops; W/D stay unbound; Q and E select.
        for code in [KeyCode::KeyW, KeyCode::KeyD] {
            assert_eq!(
                world_key_action(code, PLAIN, true),
                WorldKeyAction::Unbound,
                "{code:?} has no camera duty"
            );
        }
    }

    #[test]
    fn a_is_only_the_attack_move_arm_and_s_is_only_stop() {
        assert_eq!(
            world_key_action(KeyCode::KeyA, PLAIN, true),
            WorldKeyAction::ArmAttackMove
        );
        assert_eq!(
            world_key_action(KeyCode::KeyS, PLAIN, true),
            WorldKeyAction::Stop
        );
        // Key releases of action keys are inert (the CameraHold arms
        // above are the only release-sensitive rows).
        assert_eq!(
            world_key_action(KeyCode::KeyA, PLAIN, false),
            WorldKeyAction::Unbound
        );
        assert_eq!(
            world_key_action(KeyCode::KeyS, PLAIN, false),
            WorldKeyAction::Unbound
        );
    }

    #[test]
    fn the_tools_live_on_f10_and_f11() {
        assert_eq!(
            world_key_action(KeyCode::F10, PLAIN, true),
            WorldKeyAction::ToggleDebugOverlay
        );
        assert_eq!(
            world_key_action(KeyCode::F11, PLAIN, true),
            WorldKeyAction::BugReport
        );
        // The F1–F8 row is vacated for the camera bookmarks (phase 4
        // binds it); F3 and F8 do nothing today.
        for code in [KeyCode::F3, KeyCode::F8] {
            assert_eq!(world_key_action(code, PLAIN, true), WorldKeyAction::Unbound);
        }
    }

    #[test]
    fn the_stable_rows_keep_their_actions() {
        assert_eq!(
            world_key_action(KeyCode::KeyP, PLAIN, true),
            WorldKeyAction::Pause
        );
        assert_eq!(
            world_key_action(KeyCode::Period, PLAIN, true),
            WorldKeyAction::SingleStep
        );
        assert_eq!(
            world_key_action(KeyCode::KeyR, PLAIN, true),
            WorldKeyAction::Rematch
        );
        assert_eq!(
            world_key_action(KeyCode::Escape, PLAIN, true),
            WorldKeyAction::Escape
        );
        assert_eq!(
            world_key_action(KeyCode::Space, PLAIN, true),
            WorldKeyAction::JumpLastEvent
        );
    }

    // ---- the phase 2 rows (PLAN-M11 §4 phase 2's acceptance) ------------

    #[test]
    fn q_and_e_are_the_selection_keys() {
        assert_eq!(
            world_key_action(KeyCode::KeyE, PLAIN, true),
            WorldKeyAction::SelectSameKind
        );
        assert_eq!(
            world_key_action(KeyCode::KeyQ, PLAIN, true),
            WorldKeyAction::SelectCombat
        );
        // With modifiers they are inert (nothing else claims them yet).
        assert_eq!(
            world_key_action(KeyCode::KeyE, CTRL, true),
            WorldKeyAction::Unbound
        );
        assert_eq!(
            world_key_action(KeyCode::KeyQ, ALT, true),
            WorldKeyAction::Unbound
        );
    }

    #[test]
    fn the_digits_map_to_ten_control_groups_with_modifier_duties() {
        // Digits 1-0 are groups 1-10 (PLAN-M11 §2: "extend to ten").
        for (code, index) in [
            (KeyCode::Digit1, 0),
            (KeyCode::Digit5, 4),
            (KeyCode::Digit9, 8),
            (KeyCode::Digit0, 9),
        ] {
            assert_eq!(
                world_key_action(code, PLAIN, true),
                WorldKeyAction::RecallGroup { index },
                "{code:?} recalls group {index}"
            );
            assert_eq!(
                world_key_action(code, CTRL, true),
                WorldKeyAction::AssignGroup { index },
                "Ctrl+{code:?} assigns group {index}"
            );
            assert_eq!(
                world_key_action(code, ALT, true),
                WorldKeyAction::ViewGroup { index },
                "Alt+{code:?} views group {index}"
            );
        }
        // Ctrl beats Alt when both are held (the table's tie rule).
        assert_eq!(
            world_key_action(
                KeyCode::Digit1,
                KeyMods {
                    ctrl: true,
                    alt: true
                },
                true
            ),
            WorldKeyAction::AssignGroup { index: 0 }
        );
    }

    #[test]
    fn ctrl_arrows_cycle_units_and_workers_instead_of_scrolling() {
        assert_eq!(
            world_key_action(KeyCode::ArrowRight, CTRL, true),
            WorldKeyAction::CycleUnit { forward: true }
        );
        assert_eq!(
            world_key_action(KeyCode::ArrowLeft, CTRL, true),
            WorldKeyAction::CycleUnit { forward: false }
        );
        assert_eq!(
            world_key_action(KeyCode::ArrowUp, CTRL, true),
            WorldKeyAction::CycleWorker { forward: true }
        );
        assert_eq!(
            world_key_action(KeyCode::ArrowDown, CTRL, true),
            WorldKeyAction::CycleWorker { forward: false }
        );
        // A cycle press never starts a scroll (the held set is not
        // touched) — and the release under Ctrl still clears, so a
        // scroll that began without Ctrl cannot stick.
        assert_eq!(
            world_key_action(KeyCode::ArrowRight, CTRL, true),
            WorldKeyAction::CycleUnit { forward: true },
            "the Ctrl press is not a CameraHold"
        );
    }

    #[test]
    fn h_jumps_to_the_command_center_and_ctrl_h_is_reserved() {
        assert_eq!(
            world_key_action(KeyCode::KeyH, PLAIN, true),
            WorldKeyAction::JumpToBase
        );
        assert_eq!(
            world_key_action(KeyCode::KeyH, CTRL, true),
            WorldKeyAction::Unbound,
            "Ctrl+H stays reserved for a hero (no hero content)"
        );
    }

    // ---- the phase 3 rows (PLAN-M11 §4 phase 3's acceptance) ------------

    #[test]
    fn g_arms_guard_and_x_scatters() {
        assert_eq!(
            world_key_action(KeyCode::KeyG, PLAIN, true),
            WorldKeyAction::ArmGuard
        );
        assert_eq!(
            world_key_action(KeyCode::KeyX, PLAIN, true),
            WorldKeyAction::Scatter
        );
        // With modifiers both are inert (nothing else claims them).
        assert_eq!(
            world_key_action(KeyCode::KeyG, CTRL, true),
            WorldKeyAction::Unbound
        );
        assert_eq!(
            world_key_action(KeyCode::KeyX, ALT, true),
            WorldKeyAction::Unbound
        );
    }
}
