//! The application state machine (PLAN-M10.2 §3.1):
//! `MainMenu -> (Settings | NewMatch) -> InMatch -> (PauseMenu | EndScreen)
//! -> MainMenu`.
//!
//! Pure data and transitions — no winit types, no window, no sim. The
//! [`AppState`] carries the screen and the keyboard focus; the transition
//! functions return `(new_state, effect)` pairs where [`Effect`] is the
//! command vocabulary the app layer interprets (build/drop the match host,
//! pause it, persist settings, quit). [`NewMatchState`] owns the menu's own
//! data (mode, seed text, map index) so starting a match is a pure
//! `(screen, effect)` result the wiring layer executes verbatim.
//!
//! Settings *values* are not machine state — the app owns the live
//! [`crate::config::Settings`] and applies [`Effect::AdjustSetting`] to it,
//! which keeps one copy of the truth and lets the parser tests carry the
//! value semantics. Keyboard and mouse share the same navigation core:
//! hover sets focus, click activates, arrows + Enter work alone (§3.6's
//! "keyboard AND mouse navigable" exit bar).

use pandemonium_sim_api::{ControllerKind, MatchSetup, PlayerId, PlayerSetup};

/// The match modes the New Match screen offers (PLAN §3.3): how the two
/// match slots are driven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchMode {
    /// Today's wiring: P1 human, P2 the scripted Alpha AI.
    PlayerVsAi,
    /// Spectate: both slots driven by the Alpha AI; the human's order path
    /// is a silent no-op (no refusal cues — noise in a match the player
    /// does not own).
    AiVsAi,
    /// No opponent: P1 human, P2 present in the setup but controller-less
    /// (its starting force stands idle — "for testing").
    Sandbox,
}

impl MatchMode {
    /// The mode's label on the New Match screen (ASCII — the embedded font
    /// is an ASCII subset of DejaVu Sans).
    // (rendered by the New Match screen's commit, later this phase)
    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            Self::PlayerVsAi => "Player vs AI",
            Self::AiVsAi => "AI vs AI (spectate)",
            Self::Sandbox => "Sandbox (no opponent)",
        }
    }

    /// Whether the human's order path may submit commands in this mode —
    /// spectate silences it at the orders layer (PLAN §3.3's mode mapping;
    /// a hidden-but-live path would inject P1 commands into a match the
    /// "player" does not own).
    pub fn human_orders(self) -> bool {
        !matches!(self, Self::AiVsAi)
    }

    /// The match setup for this mode (A-112): both slots are always present
    /// — the sim's match rules and the spawn/population paths assume a
    /// two-player setup, so "no opponent" means a controller-less P2 whose
    /// starting force stands idle, never a missing player.
    pub fn match_setup(self, seed: u64) -> MatchSetup {
        let ai = |slot_is_ai: bool| {
            if slot_is_ai {
                ControllerKind::Ai
            } else {
                ControllerKind::Human
            }
        };
        MatchSetup {
            seed,
            players: vec![
                PlayerSetup {
                    player: PlayerId(0),
                    controller: ai(matches!(self, Self::AiVsAi)),
                },
                PlayerSetup {
                    player: PlayerId(1),
                    controller: ai(!matches!(self, Self::Sandbox)),
                },
            ],
        }
    }

    /// Which slots the wiring layer attaches a scripted Alpha controller
    /// to (A-112): the opponent in Player vs AI, both players in spectate,
    /// nobody in Sandbox (P2's force idles).
    pub fn ai_slots(self) -> Vec<PlayerId> {
        match self {
            Self::PlayerVsAi => vec![PlayerId(1)],
            Self::AiVsAi => vec![PlayerId(0), PlayerId(1)],
            Self::Sandbox => Vec::new(),
        }
    }

    /// Cycles to the next/previous mode (wraps — A-110).
    pub fn cycle(self, dir: i32) -> Self {
        match (self, dir.is_positive()) {
            (Self::PlayerVsAi, true) | (Self::Sandbox, false) => Self::AiVsAi,
            (Self::AiVsAi, true) | (Self::PlayerVsAi, false) => Self::Sandbox,
            (Self::Sandbox, true) | (Self::AiVsAi, false) => Self::PlayerVsAi,
        }
    }
}

/// The New Match screen's editable fields (PLAN §3.3): mode, seed, map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewMatchState {
    /// The selected match mode.
    pub mode: MatchMode,
    /// The seed field's text — digits only, edited in place. Parsed on
    /// Start; an empty or unparseable field falls back to the context's
    /// fresh random seed (the plan's "random by default").
    pub seed_text: String,
    /// The selected map's index into the bundle's map list.
    pub map_index: usize,
    /// How many maps the bundle lists (drives the wrap).
    pub map_count: usize,
}

impl NewMatchState {
    /// The seed Start will use: the parsed field, else the context's fresh
    /// random seed.
    pub fn seed_value(&self, random_seed: u64) -> u64 {
        self.seed_text.parse::<u64>().unwrap_or(random_seed)
    }

    /// Cycles the map index (wraps; a one-map bundle cycles to itself).
    pub fn cycle_map(&mut self, dir: i32) {
        if self.map_count <= 1 {
            return;
        }
        if dir.is_positive() {
            self.map_index = (self.map_index + 1) % self.map_count;
        } else {
            self.map_index = (self.map_index + self.map_count - 1) % self.map_count;
        }
    }
}

/// Where the Settings screen returns to when it closes (the plan's diagram
/// has settings reachable from both the main menu and the pause menu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SettingsReturn {
    /// Closed back to the main menu.
    MainMenu,
    /// Closed back to the (still paused) pause menu.
    PauseMenu,
}

/// One application screen.
// The variant `EndScreen` is the plan's own diagram name (PLAN-M10.2 §3.1)
// — kept verbatim over the pedantic suffix lint.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// New Match / Settings / Quit.
    MainMenu,
    /// Mode / seed / map / Start.
    NewMatch(NewMatchState),
    /// The seven settings rows + Done. The *values* live in the app's
    /// [`crate::config::Settings`]; the machine only navigates.
    Settings {
        /// Which screen Settings was opened from.
        return_to: SettingsReturn,
    },
    /// A match is running and owns the world input.
    InMatch,
    /// Resume / Settings / Restart / Quit to Menu.
    PauseMenu,
    /// The match resolved: Rematch / back to the menu.
    EndScreen,
}

/// The side effects the app layer executes (PLAN §3.1: "effects are an
/// enum the app layer interprets").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Nothing to do.
    None,
    /// Build a match host for this mode, seed, and map and enter it.
    StartMatch {
        /// The selected mode.
        mode: MatchMode,
        /// The seed the match will run (parsed field or fresh random).
        seed: u64,
        /// The selected map's index into the bundle's map list.
        map_index: usize,
    },
    /// Drop the host and rebuild the same match (same mode/seed/map — A15).
    RestartMatch,
    /// Drop the host and return to the menu.
    DropMatch,
    /// Pause the host (opening the pause menu).
    Pause,
    /// Unpause the host (leaving the pause menu).
    Unpause,
    /// Persist the settings (the settings screen's Done).
    SaveSettings,
    /// Step the focused setting's value (the app applies it to its live
    /// copy); `row` is the settings row index, `dir` the direction.
    AdjustSetting {
        /// Which settings row (see [`crate::config::SETTING_KEYS`]).
        row: usize,
        /// +1 next value, -1 previous.
        dir: i32,
    },
    /// Close the application.
    QuitApp,
}

/// The machine's view of one input event (winit keys map here — the pure
/// module never touches window types).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NavKey {
    /// Focus up one row.
    Up,
    /// Focus down one row.
    Down,
    /// Previous value on a field row (or nothing on an action row).
    Left,
    /// Next value on a field row (or nothing on an action row).
    Right,
    /// Activate the focused row.
    Enter,
    /// Back out one level.
    Escape,
    /// Delete the seed field's last digit.
    Backspace,
    /// Type a digit into the seed field (when focused).
    Digit(char),
}

/// The per-event context the caller supplies (the machine stays pure; the
/// random seed is an *input*, so tests pin it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyContext {
    /// A fresh random seed from the OS entropy source (std's hash-map
    /// seed, A-111 — never the sim's deterministic RNG). Used to fill an
    /// empty seed field and to randomize on demand.
    pub random_seed: u64,
    /// The `--seed N` flag's value (None = the seed field starts empty,
    /// i.e. random at Start) — pre-fills the New Match screen's seed field
    /// every time the screen opens.
    pub seed_prefill: Option<u64>,
    /// How many maps the loaded content tree lists.
    pub map_count: usize,
}

/// One focusable row's identity — the shared vocabulary between the
/// machine's focus model and the rendering/hit-test layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonId {
    /// Main menu: open the New Match screen.
    NewMatch,
    /// Main menu: open the settings screen.
    Settings,
    /// Main menu: quit the application.
    Quit,
    /// New Match screen: the mode field.
    Mode,
    /// New Match screen: the seed field.
    Seed,
    /// New Match screen: the map field.
    Map,
    /// New Match screen: start the match.
    Start,
    /// New Match screen: back to the main menu.
    Back,
    /// Settings screen: one settings row (by index).
    Setting(usize),
    /// Settings screen: save and return.
    Done,
    /// Pause menu: resume the match.
    Resume,
    /// Pause menu: restart the match.
    PauseRestart,
    /// Pause menu: quit to the main menu.
    QuitToMenu,
    /// End screen: restart (rematch).
    Rematch,
    /// End screen: back to the main menu.
    ToMenu,
}

/// The application state: the current screen plus the keyboard focus
/// within it. `InMatch` carries no focus (the match owns input).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppState {
    /// The current screen.
    pub screen: Screen,
    /// The focused row's index into the current screen's button list.
    pub focus: usize,
}

impl AppState {
    /// The state the app starts in.
    pub fn at_main_menu() -> Self {
        Self {
            screen: Screen::MainMenu,
            focus: 0,
        }
    }

    /// Whether a menu owns the input (world input is live only in
    /// `InMatch`; the pause menu and the end screen sit over a live match
    /// but swallow the world's clicks and keys).
    pub fn takes_world_input(&self) -> bool {
        matches!(self.screen, Screen::InMatch)
    }

    /// The screen's name for evidence lines (lowercase, ASCII).
    pub fn name(&self) -> &'static str {
        match self.screen {
            Screen::MainMenu => "main menu",
            Screen::NewMatch(_) => "new match screen",
            Screen::Settings { .. } => "settings screen",
            Screen::InMatch => "match",
            Screen::PauseMenu => "pause menu",
            Screen::EndScreen => "end screen",
        }
    }

    /// The focused row's identity.
    pub fn focused_button(&self) -> Option<ButtonId> {
        self.button_at(self.focus)
    }

    /// The row identity at a focus index (None when out of range).
    pub fn button_at(&self, index: usize) -> Option<ButtonId> {
        Some(match (&self.screen, index) {
            (Screen::MainMenu, 0) => ButtonId::NewMatch,
            (Screen::MainMenu, 1) => ButtonId::Settings,
            (Screen::MainMenu, 2) => ButtonId::Quit,
            (Screen::NewMatch(_), 0) => ButtonId::Mode,
            (Screen::NewMatch(_), 1) => ButtonId::Seed,
            (Screen::NewMatch(_), 2) => ButtonId::Map,
            (Screen::NewMatch(_), 3) => ButtonId::Start,
            (Screen::NewMatch(_), 4) => ButtonId::Back,
            (Screen::Settings { .. }, index) => {
                const SETTINGS_ROWS: usize = 7; // the seven keys + Done
                if index < SETTINGS_ROWS {
                    ButtonId::Setting(index)
                } else if index == SETTINGS_ROWS {
                    ButtonId::Done
                } else {
                    return None;
                }
            }
            (Screen::InMatch, _) => return None,
            (Screen::PauseMenu, 0) => ButtonId::Resume,
            (Screen::PauseMenu, 1) => ButtonId::Settings,
            (Screen::PauseMenu, 2) => ButtonId::PauseRestart,
            (Screen::PauseMenu, 3) => ButtonId::QuitToMenu,
            (Screen::EndScreen, 0) => ButtonId::Rematch,
            (Screen::EndScreen, 1) => ButtonId::ToMenu,
            _ => return None,
        })
    }

    /// How many focusable rows the current screen has.
    pub fn button_count(&self) -> usize {
        match self.screen {
            Screen::MainMenu => 3,
            Screen::NewMatch(_) => 5,
            Screen::Settings { .. } => 8, // 7 settings rows + Done
            Screen::InMatch => 0,
            Screen::PauseMenu => 4,
            Screen::EndScreen => 2,
        }
    }

    /// Moves focus to an index, clamped into the screen's range (mouse
    /// hover uses this; a stale index cannot escape).
    pub fn focus_index(&mut self, index: usize) {
        let count = self.button_count();
        if count > 0 {
            self.focus = index.min(count - 1);
        } else {
            self.focus = 0;
        }
    }

    /// Focus down one row (wraps — A-110).
    pub fn focus_next(&mut self) {
        let count = self.button_count();
        if count > 0 {
            self.focus = (self.focus + 1) % count;
        }
    }

    /// Focus up one row (wraps — A-110).
    pub fn focus_prev(&mut self) {
        let count = self.button_count();
        if count > 0 {
            self.focus = (self.focus + count - 1) % count;
        }
    }

    /// The Escape rung for a menu screen: back out one level. `InMatch`
    /// never reaches this (the input layer's ladder owns in-match Escape);
    /// the end screen has no back (an explicit choice is required).
    fn on_back(&self) -> (AppState, Effect) {
        let mut next = self.clone();
        next.focus = 0;
        match self.screen {
            Screen::MainMenu => (next, Effect::QuitApp),
            Screen::NewMatch(_) => {
                next.screen = Screen::MainMenu;
                (next, Effect::None)
            }
            Screen::Settings { return_to } => {
                // Esc leaves WITHOUT saving (Done is the save path).
                next.screen = match return_to {
                    SettingsReturn::MainMenu => Screen::MainMenu,
                    SettingsReturn::PauseMenu => Screen::PauseMenu,
                };
                (next, Effect::None)
            }
            Screen::InMatch | Screen::EndScreen => (next, Effect::None),
            // Esc on the pause menu resumes the match (the menu's own
            // back-out: Resume and Escape are the same edge).
            Screen::PauseMenu => {
                next.screen = Screen::InMatch;
                (next, Effect::Unpause)
            }
        }
    }

    /// Activates the row at `index` (Enter on the focused row, or a mouse
    /// click — the same core). Returns the successor state and the effect
    /// to execute.
    pub fn activate_index(&self, index: usize, ctx: &KeyContext) -> (AppState, Effect) {
        let Some(id) = self.button_at(index) else {
            return (self.clone(), Effect::None);
        };
        let mut next = self.clone();
        next.focus = index;
        match id {
            ButtonId::NewMatch => {
                // A fresh New Match screen: the default mode, the flag's
                // pre-filled seed (empty = random at Start), the first map.
                next.screen = Screen::NewMatch(NewMatchState {
                    mode: MatchMode::PlayerVsAi,
                    seed_text: ctx
                        .seed_prefill
                        .map(|seed| seed.to_string())
                        .unwrap_or_default(),
                    map_index: 0,
                    map_count: ctx.map_count.max(1),
                });
                (next, Effect::None)
            }
            ButtonId::Settings => {
                next.screen = Screen::Settings {
                    return_to: match self.screen {
                        Screen::PauseMenu => SettingsReturn::PauseMenu,
                        _ => SettingsReturn::MainMenu,
                    },
                };
                (next, Effect::None)
            }
            ButtonId::Quit => (next, Effect::QuitApp),
            ButtonId::Mode => {
                if let Screen::NewMatch(fields) = &mut next.screen {
                    fields.mode = fields.mode.cycle(1);
                }
                (next, Effect::None)
            }
            ButtonId::Seed => {
                if let Screen::NewMatch(fields) = &mut next.screen {
                    fields.seed_text = ctx.random_seed.to_string();
                }
                (next, Effect::None)
            }
            ButtonId::Map => {
                if let Screen::NewMatch(fields) = &mut next.screen {
                    fields.cycle_map(1);
                }
                (next, Effect::None)
            }
            ButtonId::Start => {
                let Screen::NewMatch(fields) = &self.screen else {
                    return (next, Effect::None);
                };
                let effect = Effect::StartMatch {
                    mode: fields.mode,
                    seed: fields.seed_value(ctx.random_seed),
                    map_index: fields.map_index,
                };
                next.screen = Screen::InMatch;
                (next, effect)
            }
            ButtonId::Back => {
                next.screen = Screen::MainMenu;
                (next, Effect::None)
            }
            ButtonId::Setting(row) => (
                next,
                Effect::AdjustSetting {
                    row,
                    dir: 1, // a click advances one value (keyboard gets Left/Right)
                },
            ),
            ButtonId::Done => {
                next.screen = match self.screen {
                    Screen::Settings { return_to } => match return_to {
                        SettingsReturn::MainMenu => Screen::MainMenu,
                        SettingsReturn::PauseMenu => Screen::PauseMenu,
                    },
                    _ => Screen::MainMenu,
                };
                (next, Effect::SaveSettings)
            }
            ButtonId::Resume => {
                next.screen = Screen::InMatch;
                (next, Effect::Unpause)
            }
            ButtonId::PauseRestart => {
                next.screen = Screen::InMatch;
                (next, Effect::RestartMatch)
            }
            ButtonId::QuitToMenu => {
                next.screen = Screen::MainMenu;
                (next, Effect::DropMatch)
            }
            ButtonId::Rematch => {
                next.screen = Screen::InMatch;
                (next, Effect::RestartMatch)
            }
            ButtonId::ToMenu => {
                next.screen = Screen::MainMenu;
                (next, Effect::DropMatch)
            }
        }
    }

    /// Activates the focused row (the keyboard path over
    /// [`AppState::activate_index`]).
    pub fn activate_focused(&self, ctx: &KeyContext) -> (AppState, Effect) {
        self.activate_index(self.focus, ctx)
    }

    /// One navigation key. The complete keyboard semantics; `InMatch`
    /// returns unchanged (the input layer's own handlers own the match's
    /// keys — the machine is not consulted there).
    pub fn on_key(&self, key: NavKey, ctx: &KeyContext) -> (AppState, Effect) {
        let mut next = self.clone();
        match key {
            NavKey::Up => {
                next.focus_prev();
                (next, Effect::None)
            }
            NavKey::Down => {
                next.focus_next();
                (next, Effect::None)
            }
            NavKey::Left | NavKey::Right => {
                let dir = if matches!(key, NavKey::Right) { 1 } else { -1 };
                match next.focused_button() {
                    Some(ButtonId::Mode) => {
                        if let Screen::NewMatch(fields) = &mut next.screen {
                            fields.mode = fields.mode.cycle(dir);
                        }
                        (next, Effect::None)
                    }
                    Some(ButtonId::Map) => {
                        if let Screen::NewMatch(fields) = &mut next.screen {
                            fields.cycle_map(dir);
                        }
                        (next, Effect::None)
                    }
                    Some(ButtonId::Setting(row)) => (
                        next,
                        Effect::AdjustSetting {
                            row,
                            dir: if matches!(key, NavKey::Right) { 1 } else { -1 },
                        },
                    ),
                    _ => (next, Effect::None), // action rows and the seed: Left/Right are inert
                }
            }
            NavKey::Enter => self.activate_focused(ctx),
            NavKey::Escape => self.on_back(),
            NavKey::Backspace => {
                if matches!(next.focused_button(), Some(ButtonId::Seed)) {
                    if let Screen::NewMatch(fields) = &mut next.screen {
                        fields.seed_text.pop();
                    }
                }
                (next, Effect::None)
            }
            NavKey::Digit(d) => {
                if matches!(next.focused_button(), Some(ButtonId::Seed)) && d.is_ascii_digit() {
                    if let Screen::NewMatch(fields) = &mut next.screen {
                        if fields.seed_text.len() < 20 {
                            fields.seed_text.push(d);
                        }
                    }
                }
                (next, Effect::None)
            }
        }
    }

    /// The in-match Escape edge (the input layer's ladder exhausted): open
    /// the pause menu and pause the host. The wiring layer calls this; the
    /// effect pauses through the existing `MatchHost` pause.
    pub fn open_pause(&self) -> (AppState, Effect) {
        let mut next = self.clone();
        if matches!(self.screen, Screen::InMatch) {
            next.screen = Screen::PauseMenu;
            next.focus = 0;
            (next, Effect::Pause)
        } else {
            (next, Effect::None)
        }
    }

    /// The match resolved: promote InMatch to the end screen (the wiring
    /// layer calls this once the host reports an outcome).
    pub fn promote_to_end(&self) -> AppState {
        let mut next = self.clone();
        if matches!(self.screen, Screen::InMatch) {
            next.screen = Screen::EndScreen;
            next.focus = 0;
        }
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pinned context (the random seed is an input — tests know it).
    fn ctx() -> KeyContext {
        KeyContext {
            random_seed: 1234,
            seed_prefill: None,
            map_count: 1,
        }
    }

    /// A pinned context carrying the `--seed 4242` prefill and 3 maps.
    fn ctx_prefilled() -> KeyContext {
        KeyContext {
            random_seed: 1234,
            seed_prefill: Some(4242),
            map_count: 3,
        }
    }

    /// The New Match screen a context produces (the activation path's
    /// builder — what a caller uses instead of hand-assembling the state).
    fn new_match_screen(ctx: &KeyContext) -> AppState {
        menu().activate_index(0, ctx).0
    }

    fn at(screen: Screen) -> AppState {
        AppState { screen, focus: 0 }
    }

    fn menu() -> AppState {
        AppState::at_main_menu()
    }

    // ---- the plan's diagram, edge by edge -------------------------------

    #[test]
    fn main_menu_new_match_opens_the_screen() {
        let (next, effect) = menu().activate_index(0, &ctx());
        assert!(matches!(next.screen, Screen::NewMatch(_)));
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn main_menu_settings_opens_settings_returning_to_the_menu() {
        let (next, effect) = menu().activate_index(1, &ctx());
        assert_eq!(
            next.screen,
            Screen::Settings {
                return_to: SettingsReturn::MainMenu
            }
        );
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn main_menu_quit_quits() {
        let (_, effect) = menu().activate_index(2, &ctx());
        assert_eq!(effect, Effect::QuitApp);
    }

    #[test]
    fn new_match_start_starts_the_match_with_the_fields() {
        let start = new_match_screen(&KeyContext {
            seed_prefill: Some(42),
            ..ctx()
        });
        let (next, effect) = start.activate_index(3, &ctx()); // Start
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(
            effect,
            Effect::StartMatch {
                mode: MatchMode::PlayerVsAi,
                seed: 42,
                map_index: 0
            }
        );
    }

    #[test]
    fn new_match_back_and_escape_return_to_the_main_menu() {
        let start = new_match_screen(&ctx());
        let (next, _) = start.activate_index(4, &ctx()); // Back
        assert_eq!(next.screen, Screen::MainMenu);
        let (next, _) = start.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::MainMenu);
    }

    #[test]
    fn an_empty_seed_field_starts_from_the_fresh_random_seed() {
        let start = new_match_screen(&ctx());
        let fields = match &start.screen {
            Screen::NewMatch(fields) => fields.clone(),
            _ => unreachable!(),
        };
        assert_eq!(fields.seed_text, "");
        assert_eq!(fields.seed_value(ctx().random_seed), 1234);
        let (_, effect) = start.activate_index(3, &ctx());
        assert_eq!(
            effect,
            Effect::StartMatch {
                mode: MatchMode::PlayerVsAi,
                seed: 1234,
                map_index: 0
            }
        );
    }

    #[test]
    fn settings_done_saves_and_returns_where_it_was_opened_from() {
        // From the main menu.
        let settings = at(Screen::Settings {
            return_to: SettingsReturn::MainMenu,
        });
        let (next, effect) = settings.activate_index(7, &ctx()); // Done
        assert_eq!(effect, Effect::SaveSettings);
        assert_eq!(next.screen, Screen::MainMenu);
        // From the pause menu (the awkward edge: settings must remember).
        let settings = at(Screen::Settings {
            return_to: SettingsReturn::PauseMenu,
        });
        let (next, effect) = settings.activate_index(7, &ctx());
        assert_eq!(effect, Effect::SaveSettings);
        assert_eq!(next.screen, Screen::PauseMenu);
    }

    #[test]
    fn settings_escape_returns_without_saving() {
        let settings = at(Screen::Settings {
            return_to: SettingsReturn::PauseMenu,
        });
        let (next, effect) = settings.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_ne!(effect, Effect::SaveSettings);
    }

    #[test]
    fn in_match_escape_opens_the_pause_menu_and_pauses() {
        let (next, effect) = at(Screen::InMatch).open_pause();
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::Pause);
    }

    #[test]
    fn pause_menu_resume_and_escape_resume_the_match() {
        let paused = at(Screen::PauseMenu);
        let (next, effect) = paused.activate_index(0, &ctx()); // Resume
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(effect, Effect::Unpause);
        let (next, effect) = paused.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(effect, Effect::Unpause);
    }

    #[test]
    fn pause_menu_settings_opens_settings_returning_to_the_pause_menu() {
        let (next, _) = at(Screen::PauseMenu).activate_index(1, &ctx());
        assert_eq!(
            next.screen,
            Screen::Settings {
                return_to: SettingsReturn::PauseMenu
            }
        );
    }

    #[test]
    fn pause_menu_restart_rebuilds_and_reenters_the_match() {
        let (next, effect) = at(Screen::PauseMenu).activate_index(2, &ctx());
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(effect, Effect::RestartMatch);
    }

    #[test]
    fn pause_menu_quit_to_menu_drops_the_match() {
        let (next, effect) = at(Screen::PauseMenu).activate_index(3, &ctx());
        assert_eq!(next.screen, Screen::MainMenu);
        assert_eq!(effect, Effect::DropMatch);
    }

    #[test]
    fn the_match_end_promotes_in_match_to_the_end_screen() {
        let next = at(Screen::InMatch).promote_to_end();
        assert_eq!(next.screen, Screen::EndScreen);
        // Promoting a non-match screen changes nothing.
        let unchanged = at(Screen::MainMenu).promote_to_end();
        assert_eq!(unchanged.screen, Screen::MainMenu);
    }

    #[test]
    fn the_end_screen_offers_rematch_and_menu() {
        let end = at(Screen::EndScreen);
        let (next, effect) = end.activate_index(0, &ctx()); // Rematch
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(effect, Effect::RestartMatch);
        let (next, effect) = end.activate_index(1, &ctx()); // ToMenu
        assert_eq!(next.screen, Screen::MainMenu);
        assert_eq!(effect, Effect::DropMatch);
        // No Escape rung on the end screen: an explicit choice is required.
        let (same, effect) = end.on_key(NavKey::Escape, &ctx());
        assert_eq!(same.screen, Screen::EndScreen);
        assert_eq!(effect, Effect::None);
    }

    // ---- the focus model -------------------------------------------------

    #[test]
    fn focus_moves_wrapping_within_the_screen() {
        let mut state = menu();
        assert_eq!(state.focused_button(), Some(ButtonId::NewMatch));
        state.focus_next();
        assert_eq!(state.focused_button(), Some(ButtonId::Settings));
        state.focus_next();
        assert_eq!(state.focused_button(), Some(ButtonId::Quit));
        state.focus_next(); // wraps (A-110)
        assert_eq!(state.focused_button(), Some(ButtonId::NewMatch));
        state.focus_prev(); // wraps back
        assert_eq!(state.focused_button(), Some(ButtonId::Quit));
    }

    #[test]
    fn keyboard_navigation_and_activation_reach_every_button() {
        let mut state = menu();
        for _ in 0..2 {
            state.focus_next();
        }
        assert_eq!(state.focused_button(), Some(ButtonId::Quit));
        // Enter activates the focused row — the Quit row.
        let (_, effect) = state.on_key(NavKey::Enter, &ctx());
        assert_eq!(effect, Effect::QuitApp);
    }

    #[test]
    fn mouse_hover_clamps_into_the_screen_and_click_activates() {
        let mut state = menu();
        state.focus_index(99); // a stale index cannot escape
        assert_eq!(state.focus, 2);
        state.focus_index(1);
        let (_, effect) = state.activate_index(1, &ctx()); // a click on Settings
        assert!(matches!(effect, Effect::None));
        assert_eq!(state.screen, Screen::MainMenu);
    }

    #[test]
    fn an_out_of_range_click_is_a_no_op() {
        let (next, effect) = menu().activate_index(17, &ctx());
        assert_eq!(next, menu());
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn in_match_has_no_focusable_rows_and_world_input_is_live() {
        let state = at(Screen::InMatch);
        assert_eq!(state.button_count(), 0);
        assert!(state.takes_world_input());
        // Every menu screen swallows the world's input.
        assert!(!menu().takes_world_input());
        assert!(!at(Screen::PauseMenu).takes_world_input());
        assert!(!at(Screen::EndScreen).takes_world_input());
    }

    // ---- the New Match fields --------------------------------------------

    #[test]
    fn mode_cycles_through_all_three_modes_both_ways() {
        assert_eq!(MatchMode::PlayerVsAi.cycle(1), MatchMode::AiVsAi);
        assert_eq!(MatchMode::AiVsAi.cycle(1), MatchMode::Sandbox);
        assert_eq!(MatchMode::Sandbox.cycle(1), MatchMode::PlayerVsAi);
        assert_eq!(MatchMode::PlayerVsAi.cycle(-1), MatchMode::Sandbox);
        assert_eq!(MatchMode::Sandbox.cycle(-1), MatchMode::AiVsAi);
        assert_eq!(MatchMode::AiVsAi.cycle(-1), MatchMode::PlayerVsAi);
    }

    #[test]
    fn left_right_on_the_mode_row_cycles_the_field() {
        let start = new_match_screen(&ctx());
        let mode_row = {
            let mut s = start.clone();
            s.focus_index(0);
            s
        };
        let (next, _) = mode_row.on_key(NavKey::Right, &ctx());
        assert_eq!(
            next.screen,
            Screen::NewMatch(NewMatchState {
                mode: MatchMode::AiVsAi,
                seed_text: String::new(),
                map_index: 0,
                map_count: 1,
            })
        );
        let (back, _) = mode_row.on_key(NavKey::Left, &ctx());
        assert_eq!(
            back.screen,
            Screen::NewMatch(NewMatchState {
                mode: MatchMode::Sandbox,
                seed_text: String::new(),
                map_index: 0,
                map_count: 1,
            })
        );
    }

    #[test]
    fn the_map_row_cycles_and_wraps_by_the_bundle_count() {
        let start = new_match_screen(&KeyContext {
            map_count: 3,
            ..ctx()
        });
        let mut map_row = start.clone();
        map_row.focus_index(2);
        let at_focus = |screen: Screen| AppState { screen, focus: 2 };
        let (next, _) = map_row.on_key(NavKey::Right, &ctx());
        assert!(matches!(
            &next.screen,
            Screen::NewMatch(f) if f.map_index == 1
        ));
        let (next, _) = at_focus(next.screen).on_key(NavKey::Right, &ctx());
        assert!(matches!(
            &next.screen,
            Screen::NewMatch(f) if f.map_index == 2
        ));
        let (next, _) = at_focus(next.screen).on_key(NavKey::Right, &ctx());
        assert!(matches!(
            &next.screen,
            Screen::NewMatch(f) if f.map_index == 0 // wrapped
        ));
        // A one-map bundle stays on its only map.
        let single = new_match_screen(&ctx());
        let (same, _) = single.on_key(NavKey::Right, &ctx());
        assert!(matches!(&same.screen, Screen::NewMatch(f) if f.map_index == 0));
    }

    #[test]
    fn seed_digits_type_backspace_deletes_and_enter_randomizes() {
        let start = new_match_screen(&ctx());
        let mut seed_row = start.clone();
        seed_row.focus_index(1);
        let (mut next, _) = seed_row.on_key(NavKey::Digit('4'), &ctx());
        for d in ['2', '0', '6', '9'] {
            let (n, _) = next.on_key(NavKey::Digit(d), &ctx());
            next = n;
        }
        assert!(matches!(&next.screen, Screen::NewMatch(f) if f.seed_text == "42069"));
        let (next, _) = next.on_key(NavKey::Backspace, &ctx());
        assert!(matches!(&next.screen, Screen::NewMatch(f) if f.seed_text == "4206"));
        let (next, _) = next.on_key(NavKey::Enter, &ctx()); // randomize
        assert!(matches!(&next.screen, Screen::NewMatch(f) if f.seed_text == "1234"));
        // The typed seed is what Start uses.
        let (start_effect, _) = next.on_key(NavKey::Enter, &ctx());
        let _ = start_effect;
        let typed = AppState {
            screen: Screen::NewMatch(NewMatchState {
                mode: MatchMode::PlayerVsAi,
                seed_text: "42069".to_string(),
                map_index: 0,
                map_count: 1,
            }),
            focus: 3,
        };
        let (_, effect) = typed.on_key(NavKey::Enter, &ctx());
        assert_eq!(
            effect,
            Effect::StartMatch {
                mode: MatchMode::PlayerVsAi,
                seed: 42069,
                map_index: 0
            }
        );
    }

    #[test]
    fn seed_typing_is_capped_and_digits_only_land_when_the_row_is_focused() {
        let start = new_match_screen(&ctx());
        let mut seed_row = start.clone();
        seed_row.focus_index(1);
        let mut next = seed_row.clone();
        for _ in 0..30 {
            let (n, _) = next.on_key(NavKey::Digit('7'), &ctx());
            next = n;
        }
        assert!(matches!(&next.screen, Screen::NewMatch(f) if f.seed_text.len() == 20));
        // A digit with the Mode row focused types nothing.
        let mut mode_row = start.clone();
        mode_row.focus_index(0);
        let (n, _) = mode_row.on_key(NavKey::Digit('7'), &ctx());
        assert!(matches!(&n.screen, Screen::NewMatch(f) if f.seed_text.is_empty()));
    }

    #[test]
    fn a_prefilled_seed_field_carries_the_seed_flag_value() {
        let start = new_match_screen(&ctx_prefilled());
        assert!(matches!(&start.screen, Screen::NewMatch(f) if f.seed_text == "4242"));
        let (_, effect) = start.activate_index(3, &ctx());
        assert_eq!(
            effect,
            Effect::StartMatch {
                mode: MatchMode::PlayerVsAi,
                seed: 4242,
                map_index: 0
            }
        );
    }

    // ---- the settings rows -----------------------------------------------

    #[test]
    fn settings_left_right_and_click_adjust_the_focused_row() {
        let mut settings = at(Screen::Settings {
            return_to: SettingsReturn::MainMenu,
        });
        settings.focus_index(2); // zoom_min
        let (_, effect) = settings.on_key(NavKey::Right, &ctx());
        assert_eq!(effect, Effect::AdjustSetting { row: 2, dir: 1 });
        let (_, effect) = settings.on_key(NavKey::Left, &ctx());
        assert_eq!(effect, Effect::AdjustSetting { row: 2, dir: -1 });
        let (_, effect) = settings.activate_index(2, &ctx()); // a click
        assert_eq!(effect, Effect::AdjustSetting { row: 2, dir: 1 });
        // Left/Right on the Done row adjusts nothing.
        let mut done = at(Screen::Settings {
            return_to: SettingsReturn::MainMenu,
        });
        done.focus_index(7);
        let (_, effect) = done.on_key(NavKey::Left, &ctx());
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn the_settings_screen_lists_seven_rows_plus_done() {
        let settings = at(Screen::Settings {
            return_to: SettingsReturn::MainMenu,
        });
        assert_eq!(settings.button_count(), 8);
        for row in 0..7 {
            assert_eq!(
                settings.button_at(row),
                Some(ButtonId::Setting(row)),
                "row {row}"
            );
        }
        assert_eq!(settings.button_at(7), Some(ButtonId::Done));
        assert_eq!(settings.button_at(8), None);
    }

    // ---- mode semantics ----------------------------------------------------

    #[test]
    fn spectate_is_the_only_mode_where_the_human_cannot_order() {
        assert!(MatchMode::PlayerVsAi.human_orders());
        assert!(!MatchMode::AiVsAi.human_orders());
        assert!(MatchMode::Sandbox.human_orders());
    }

    #[test]
    fn the_mode_labels_are_ascii() {
        // The embedded font is an ASCII subset (the Phase 3 trap T8).
        assert!(MatchMode::PlayerVsAi.label().is_ascii());
        assert!(MatchMode::AiVsAi.label().is_ascii());
        assert!(MatchMode::Sandbox.label().is_ascii());
    }

    // ---- mode -> setup/controller mapping (A-112) -------------------------

    #[test]
    fn player_vs_ai_is_todays_wiring_one_human_one_ai() {
        let setup = MatchMode::PlayerVsAi.match_setup(42);
        assert_eq!(setup.seed, 42);
        assert_eq!(setup.players.len(), 2);
        assert_eq!(setup.players[0].player, PlayerId(0));
        assert_eq!(setup.players[0].controller, ControllerKind::Human);
        assert_eq!(setup.players[1].player, PlayerId(1));
        assert_eq!(setup.players[1].controller, ControllerKind::Ai);
        assert_eq!(MatchMode::PlayerVsAi.ai_slots(), vec![PlayerId(1)]);
    }

    #[test]
    fn spectate_drives_both_slots_and_yields_them_to_the_ai() {
        let setup = MatchMode::AiVsAi.match_setup(7);
        assert_eq!(setup.players[0].controller, ControllerKind::Ai);
        assert_eq!(setup.players[1].controller, ControllerKind::Ai);
        assert_eq!(MatchMode::AiVsAi.ai_slots(), vec![PlayerId(0), PlayerId(1)]);
    }

    #[test]
    fn sandbox_keeps_both_players_but_attaches_no_controller() {
        // P2 stays in the setup (the defeat rule, spawns, and population all
        // assume two players — a missing P2 is not a legal setup, A-112);
        // the controller vec is empty so its starting force idles.
        let setup = MatchMode::Sandbox.match_setup(9);
        assert_eq!(setup.players.len(), 2);
        assert_eq!(setup.players[0].controller, ControllerKind::Human);
        assert_eq!(setup.players[1].controller, ControllerKind::Human);
        assert_eq!(MatchMode::Sandbox.ai_slots(), Vec::<PlayerId>::new());
    }
}
