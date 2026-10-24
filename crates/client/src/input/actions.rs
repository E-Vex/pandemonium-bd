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
    /// Numpad 4 / 6 — rotate the camera left / right (held; the same rate
    /// semantics the Q/E rotation had).
    CameraRotate { left: bool, pressed: bool },
    /// Numpad 8 / 2 — zoom in / out (held, at the named step rate).
    CameraZoom { in_: bool, pressed: bool },
    /// Numpad 5 — reset the camera to the defaults (yaw 0, default pitch,
    /// the opening distance; the target stays).
    CameraReset,
    /// F9 — toggle the bottom command bar (HUD + minimap; menus never
    /// hide; pure presentation, nothing persists).
    ToggleCommandBar,
    /// Tab — toggle the mission objective HUD panel (in plain matches the
    /// panel does not exist and the key is inert — the card says so).
    ToggleObjectives,
    /// F1–F8 — jump to camera bookmark `slot` (restores the five-tuple).
    BookmarkJump { slot: usize },
    /// Ctrl+F1–F8 — set camera bookmark `slot` to the current view.
    BookmarkSet { slot: usize },
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
    // PLAN-M11 §2 phase 4: Ctrl+F-slot sets the bookmark, plain F-slot
    // jumps to it.
    let bookmark = |slot: usize, mods: KeyMods| {
        if mods.ctrl {
            WorldKeyAction::BookmarkSet { slot }
        } else {
            WorldKeyAction::BookmarkJump { slot }
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
        // PLAN-M11 §2 phase 4: the numpad camera keys (held rotate and
        // zoom; 5 resets) and the F1–F8 bookmark row (Ctrl sets, plain
        // jumps). The tools stay on F10/F11.
        KeyCode::Numpad4 => WorldKeyAction::CameraRotate {
            left: true,
            pressed,
        },
        KeyCode::Numpad6 => WorldKeyAction::CameraRotate {
            left: false,
            pressed,
        },
        KeyCode::Numpad8 => WorldKeyAction::CameraZoom { in_: true, pressed },
        KeyCode::Numpad2 => WorldKeyAction::CameraZoom {
            in_: false,
            pressed,
        },
        KeyCode::Numpad5 if pressed => WorldKeyAction::CameraReset,
        // PLAN-M11 §2 phase 5: F9 toggles the command bar, Tab the
        // objective panel (missions only).
        KeyCode::F9 if pressed => WorldKeyAction::ToggleCommandBar,
        KeyCode::Tab if pressed => WorldKeyAction::ToggleObjectives,
        KeyCode::F1 if pressed => bookmark(0, mods),
        KeyCode::F2 if pressed => bookmark(1, mods),
        KeyCode::F3 if pressed => bookmark(2, mods),
        KeyCode::F4 if pressed => bookmark(3, mods),
        KeyCode::F5 if pressed => bookmark(4, mods),
        KeyCode::F6 if pressed => bookmark(5, mods),
        KeyCode::F7 if pressed => bookmark(6, mods),
        KeyCode::F8 if pressed => bookmark(7, mods),
        // W is reserved for air selection (no air content yet), D is
        // simply free, Ctrl+H is reserved for a hero. F3 and F8 are bound
        // as bookmark slots 3 and 8 now (the tools live on F10/F11).
        KeyCode::KeyW | KeyCode::KeyD => WorldKeyAction::Unbound,
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
        // The F1–F8 row belongs to the camera bookmarks now (phase 4
        // bound it); the tools never return to it.
        assert_ne!(
            world_key_action(KeyCode::F3, PLAIN, true),
            WorldKeyAction::Unbound
        );
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

    // ---- the phase 4 rows (PLAN-M11 §4 phase 4's acceptance) ------------

    #[test]
    fn the_numpad_camera_keys_are_held_rotate_and_zoom_plus_reset() {
        assert_eq!(
            world_key_action(KeyCode::Numpad4, PLAIN, true),
            WorldKeyAction::CameraRotate {
                left: true,
                pressed: true
            }
        );
        assert_eq!(
            world_key_action(KeyCode::Numpad6, PLAIN, true),
            WorldKeyAction::CameraRotate {
                left: false,
                pressed: true
            }
        );
        // The holds clear on release like the arrows (no stuck rotate).
        assert_eq!(
            world_key_action(KeyCode::Numpad4, PLAIN, false),
            WorldKeyAction::CameraRotate {
                left: true,
                pressed: false
            }
        );
        assert_eq!(
            world_key_action(KeyCode::Numpad8, PLAIN, true),
            WorldKeyAction::CameraZoom {
                in_: true,
                pressed: true
            }
        );
        assert_eq!(
            world_key_action(KeyCode::Numpad2, PLAIN, true),
            WorldKeyAction::CameraZoom {
                in_: false,
                pressed: true
            }
        );
        assert_eq!(
            world_key_action(KeyCode::Numpad2, PLAIN, false),
            WorldKeyAction::CameraZoom {
                in_: false,
                pressed: false
            }
        );
        assert_eq!(
            world_key_action(KeyCode::Numpad5, PLAIN, true),
            WorldKeyAction::CameraReset
        );
    }

    #[test]
    fn f9_and_tab_toggle_their_panels() {
        assert_eq!(
            world_key_action(KeyCode::F9, PLAIN, true),
            WorldKeyAction::ToggleCommandBar
        );
        assert_eq!(
            world_key_action(KeyCode::Tab, PLAIN, true),
            WorldKeyAction::ToggleObjectives
        );
    }

    #[test]
    fn f1_through_f8_are_camera_bookmarks_and_ctrl_sets_them() {
        for (code, slot) in [(KeyCode::F1, 0), (KeyCode::F4, 3), (KeyCode::F8, 7)] {
            assert_eq!(
                world_key_action(code, PLAIN, true),
                WorldKeyAction::BookmarkJump { slot },
                "{code:?} jumps to bookmark {slot}"
            );
            assert_eq!(
                world_key_action(code, CTRL, true),
                WorldKeyAction::BookmarkSet { slot },
                "Ctrl+{code:?} sets bookmark {slot}"
            );
        }
        // The tools stayed on F10/F11 (phase 1's relocation holds).
        assert_eq!(
            world_key_action(KeyCode::F10, PLAIN, true),
            WorldKeyAction::ToggleDebugOverlay
        );
        assert_eq!(
            world_key_action(KeyCode::F11, PLAIN, true),
            WorldKeyAction::BugReport
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
