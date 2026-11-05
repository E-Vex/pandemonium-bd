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

/// The mission picker screen's fields (ADR-0201 Phase 1, A-011 Task 1): the
/// mission list cycles by index exactly like the map picker
/// ([`NewMatchState::cycle_map`]'s wrap law) and shows the selected mission's
/// display name; the seed field mirrors the New Match screen's; and the New
/// Match fields the picker was opened from ride along so Back restores them
/// unchanged. The picker is a sub-screen of New Match — a menu peer, never a
/// gate (ADR-0201 §6): the advanced modes stay directly reachable exactly as
/// they were.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionPickerState {
    /// The New Match fields the picker was opened from (Back/Esc restores
    /// them unchanged).
    pub new_match: NewMatchState,
    /// The seed field's text — digits only, edited in place. Starts as the
    /// parent's seed so opening the picker loses nothing.
    pub seed_text: String,
    /// The selected mission's index into the tree's mission list.
    pub mission_index: usize,
    /// How many missions the tree lists (drives the wrap).
    pub mission_count: usize,
}

impl MissionPickerState {
    /// The seed Start will use: the parsed field, else the context's fresh
    /// random seed (the New Match screen's law, verbatim).
    pub fn seed_value(&self, random_seed: u64) -> u64 {
        self.seed_text.parse::<u64>().unwrap_or(random_seed)
    }

    /// Cycles the mission index (the map picker's wrap law: a one-mission
    /// tree cycles to itself; a mission-less tree never gets here — the
    /// Missions row is inert without a mission to pick).
    pub fn cycle_mission(&mut self, dir: i32) {
        if self.mission_count <= 1 {
            return;
        }
        if dir.is_positive() {
            self.mission_index = (self.mission_index + 1) % self.mission_count;
        } else {
            self.mission_index = (self.mission_index + self.mission_count - 1) % self.mission_count;
        }
    }
}

/// One campaign entry's recorded fate, as the campaign screen resolves it
/// (B-026, ADR-0201 §6): the two records are distinct (the progress file
/// distinguishes them), and the two derived states are the unlock rule's
/// output — `Locked` is a display-and-start gate **inside the campaign
/// screen only** (the campaign is a menu peer, never a gate: the plain
/// mission picker and every advanced mode stay exactly as reachable as
/// they were).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CampaignStatus {
    /// The unlock rule does not reach this entry yet — shown locked, not
    /// startable from the campaign screen.
    Locked,
    /// No record, and the unlock rule reaches it — startable.
    Available,
    /// The mission was won.
    Completed,
    /// The mission was skipped (replayable; a later victory upgrades the
    /// record to completed).
    Skipped,
}

impl CampaignStatus {
    /// The row's status tag on the campaign screen (ASCII — the embedded
    /// font constraint).
    pub fn tag(self) -> &'static str {
        match self {
            Self::Locked => "LOCKED",
            Self::Available => "AVAILABLE",
            Self::Completed => "COMPLETED",
            Self::Skipped => "SKIPPED",
        }
    }
}

/// One entry of the campaign screen (B-026, ADR-0201 §6): a chapter
/// mission in play order, or a challenge mission surfacing beside the
/// campaign. The snapshot is app-built from
/// `(ContentTree::campaign, CampaignProgress)` when the screen opens —
/// the machine owns the navigation and the skip bookkeeping over it, the
/// app owns the build (the machine never touches the content tree).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CampaignEntry {
    /// The mission's index into the tree's mission list — what
    /// `Effect::StartMission` carries.
    pub mission_index: usize,
    /// The mission's display name (ASCII).
    pub name: String,
    /// A challenge entry (surfaced beside the chapters; unlocks on its
    /// chapter's *completion*, A-211).
    pub challenge: bool,
    /// A challenge's chapter, as an entry index into this list (chapters
    /// occupy the leading entries in play order, so a chapter's own entry
    /// index is its position — `None` for chapters).
    pub extends: Option<usize>,
    /// The mission's progress record, if any.
    pub record: Option<crate::progress::MissionRecord>,
}

/// The campaign screen's fields (B-026, ADR-0201 §6/§9 Phase 3): the
/// campaign's entries in play order (chapters first, then the challenges
/// beside them), the selected entry, and the seed field mirroring the
/// mission picker's. The unlock rule is a method, not stored state:
/// chapter N+1 unlocks when chapter N is completed **or skipped**; a
/// challenge unlocks when its chapter is **completed** (skip unlocks the
/// next chapter, never the challenge beside it — A-211); an entry with
/// its own record is always startable (a skipped mission stays
/// replayable, and a stale file can never soft-lock a replay).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CampaignState {
    /// The campaign's display name (the screen's title).
    pub campaign_name: String,
    /// The seed field's text — digits only, edited in place (the mission
    /// picker's law verbatim).
    pub seed_text: String,
    /// The selected entry's index into `entries`.
    pub selection: usize,
    /// The campaign's entries: the chapters in play order, then the
    /// challenges. Never empty (the content gates guarantee at least one
    /// chapter when a campaign file loaded at all).
    pub entries: Vec<CampaignEntry>,
}

impl CampaignState {
    /// The seed Start will use: the parsed field, else the context's fresh
    /// random seed (the New Match screen's law, verbatim).
    pub fn seed_value(&self, random_seed: u64) -> u64 {
        self.seed_text.parse::<u64>().unwrap_or(random_seed)
    }

    /// How many entries the campaign lists (drives the wrap).
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Cycles the selection (the map picker's wrap law: a one-entry
    /// campaign cycles to itself).
    pub fn cycle_selection(&mut self, dir: i32) {
        let count = self.entries.len();
        if count <= 1 {
            return;
        }
        if dir.is_positive() {
            self.selection = (self.selection + 1) % count;
        } else {
            self.selection = (self.selection + count - 1) % count;
        }
    }

    /// Resolves an entry's status — the unlock rule over the snapshot's
    /// records (derived, recomputed on every read, never stored):
    ///
    /// - an entry with a record shows it (completed/skipped — and is
    ///   startable: the record itself unlocks a replay, so a stale or
    ///   partial progress file degrades to "replayable", never to a
    ///   soft-lock);
    /// - a chapter without a record is available iff it is the first or
    ///   its predecessor is completed-or-skipped;
    /// - a challenge without a record is available iff the chapter it
    ///   extends is completed (a skip does not unlock challenges, A-211).
    pub fn status(&self, index: usize) -> CampaignStatus {
        let Some(entry) = self.entries.get(index) else {
            return CampaignStatus::Locked;
        };
        match entry.record {
            Some(record) => match record {
                crate::progress::MissionRecord::Completed => CampaignStatus::Completed,
                crate::progress::MissionRecord::Skipped => CampaignStatus::Skipped,
            },
            None => {
                let unlocked = if entry.challenge {
                    let extends = entry.extends.unwrap_or(usize::MAX);
                    matches!(
                        self.entries.get(extends).map(|chapter| chapter.record),
                        Some(Some(crate::progress::MissionRecord::Completed))
                    )
                } else {
                    // Chapters are the leading entries in play order:
                    // index 0 is the first, and index N's predecessor is
                    // entry N-1 — a chapter, by construction.
                    index == 0
                        || matches!(
                            self.entries.get(index - 1).map(|prev| prev.record),
                            Some(Some(
                                crate::progress::MissionRecord::Completed
                                    | crate::progress::MissionRecord::Skipped
                            ))
                        )
                };
                if unlocked {
                    CampaignStatus::Available
                } else {
                    CampaignStatus::Locked
                }
            }
        }
    }

    /// Whether the entry at `index` may be started from the campaign
    /// screen (locked entries are the unlock rule's only gate — and the
    /// gate lives here, never outside this screen).
    pub fn startable(&self, index: usize) -> bool {
        !matches!(self.status(index), CampaignStatus::Locked)
    }

    /// Records a skip in the snapshot and reports whether the state
    /// changed — the machine's half of the Skip affordance (the app's
    /// half persists the same fact; both run on the same activation).
    /// Only an available entry can be skipped: a locked entry has nothing
    /// to unlock, a completed one nothing forward to unlock, and a
    /// skipped one is already skipped.
    pub fn skip(&mut self, index: usize) -> bool {
        if !matches!(self.status(index), CampaignStatus::Available) {
            return false;
        }
        self.entries[index].record = Some(crate::progress::MissionRecord::Skipped);
        true
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

/// The action a confirmation screen guards (A-009 task 2). Restart and
/// Quit to Menu both discard live match state, so they open the confirm
/// rung first; the action stores which effect Confirm runs unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfirmAction {
    /// Restart the match (the existing `Effect::RestartMatch` path).
    Restart,
    /// Quit to the main menu (the existing `Effect::DropMatch` path).
    QuitToMenu,
    /// PLAN-M11 §2 phase 5: resign the match — submits the existing,
    /// never-before-sent `CommandKind::Resign` (the vocabulary gap
    /// closes without touching the sim) and the match flows to the
    /// existing end screen.
    Resign,
    /// B-026 (ADR-0201 §6, the second skip): skip the live mission —
    /// record it as *skipped* (distinct from completed) in the campaign
    /// progress, unlock the next entry, and leave the match for the main
    /// menu. The pause menu's Skip Mission row opens this rung (a
    /// match-discarding action is confirm-gated like Restart, Resign and
    /// Quit to Menu); `mission_index` is the live mission's tree index
    /// the row captured from the context.
    SkipMission {
        /// The live mission's index into the tree's mission list.
        mission_index: usize,
    },
}

/// The window focus / minimize signals the pure auto-pause decision reads
/// (A-009 task 3). winit 0.30's `WindowEvent::Focused(bool)` is the focus
/// signal; minimize has no dedicated variant, so the pinned winit emits
/// `WindowEvent::Resized(PhysicalSize::ZERO)` on minimize (the standard
/// 0.30 signal, used here through the wiring layer's translation). The
/// pure module never touches winit types — it reads this enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FocusEvent {
    /// The window lost keyboard focus (`Focused(false)`).
    Unfocused,
    /// The window regained keyboard focus (`Focused(true)`).
    Focused,
    /// The window was minimized (`Resized((0, 0))` on the pinned winit).
    Minimized,
}

/// A-009 task 3: the auto-pause decision — a pure function over
/// `(screen, mode, is_paused, event)` that returns `Effect::Pause` only
/// when a live human-driven match is in progress and focus is lost or the
/// window minimized. Concretely:
///
/// - the screen is `InMatch` (not on a menu, not on the end screen — the
///   pause menu is already a paused state, and the end screen has no live
///   match to pause);
/// - `mode.human_orders()` is true (not `AiVsAi` spectate — a spectated
///   match is not the player's to guard, and the AI keeps running);
/// - the host is not already paused (a `P` pause stays a `P` pause —
///   regaining focus later does not auto-resume, so a focus blip during a
///   manual pause must not stack a second pause on top);
/// - the event is `Unfocused` or `Minimized`.
///
/// Regaining focus (`Focused`) never auto-resumes — the player resumes
/// explicitly through the pause menu's Resume (the classic-RTS contract;
/// the brief's design ruling).
///
/// The wiring layer applies the returned effect: `Effect::Pause` triggers
/// the existing `AppState::open_pause()` path (which both pauses the host
/// and opens the pause menu), `Effect::None` is a no-op. The
/// `--frames`/smoke/`--record` runs behave exactly as before — the
/// windowed smoke under Xvfb has no focus changes or minimize events, so
/// the function returns `Effect::None` there.
pub fn auto_pause_on_focus(
    screen: &Screen,
    mode: MatchMode,
    is_paused: bool,
    event: FocusEvent,
) -> Effect {
    use FocusEvent::{Focused, Minimized, Unfocused};
    match (screen, mode, is_paused, event) {
        (Screen::InMatch, mode, false, Unfocused | Minimized) if mode.human_orders() => {
            Effect::Pause
        }
        // Regaining focus never auto-resumes (the brief's design ruling).
        (_, _, _, Focused) => Effect::None,
        _ => Effect::None,
    }
}

/// One application screen.
// The variant `EndScreen` is the plan's own diagram name (PLAN-M10.2 §3.1)
// — kept verbatim over the pedantic suffix lint.
#[allow(clippy::enum_variant_names)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Screen {
    /// New Match / Settings / Controls / Quit.
    MainMenu,
    /// Mode / seed / map / Start.
    NewMatch(NewMatchState),
    /// Mission / seed / Start / Back — A-011 (ADR-0201 Phase 1): the
    /// mission picker, a sub-screen of New Match reached by its Missions
    /// row. Mirrors the map picker (cycles the tree's mission list, shows
    /// the selected mission's display name); Start resolves to a mission
    /// match.
    MissionPicker(MissionPickerState),
    /// The campaign screen (B-026, ADR-0201 §6/§9 Phase 3): the campaign's
    /// chapters in play order with the challenges beside them, the
    /// unlock/skip semantics, and Start/Skip on the selected entry. A
    /// menu peer of the main menu, never a gate. The state is an
    /// app-built snapshot — `Effect::OpenCampaign` builds it from
    /// `(ContentTree::campaign, CampaignProgress)`; the machine owns the
    /// navigation and the skip bookkeeping over the snapshot.
    Campaign(CampaignState),
    /// The eight settings rows + Done. The *values* live in the app's
    /// [`crate::config::Settings`]; the machine only navigates.
    Settings {
        /// Which screen Settings was opened from.
        return_to: SettingsReturn,
    },
    /// A match is running and owns the world input.
    InMatch,
    /// Resume / Settings / Controls / Restart / Quit to Menu.
    PauseMenu,
    /// The match resolved: Rematch / back to the menu.
    EndScreen,
    /// The full controls reference (A-009 task 1): the standing
    /// [`crate::report::CONTROLS_TABLE`] rendered as informational rows,
    /// plus a single Back button. `return_to` follows the Settings pattern
    /// (opened from both the main menu and the pause menu).
    Controls {
        /// Which screen Controls was opened from.
        return_to: SettingsReturn,
    },
    /// A confirmation rung (A-009 task 2): the pause menu's Restart and
    /// Quit to Menu open this screen before discarding a live match. Two
    /// rows (Cancel / Confirm), default focus on Cancel so a mis-press
    /// (Enter on the focused row, or Esc) cancels. Confirm runs the
    /// pending [`ConfirmAction`] unchanged through the existing effect
    /// path.
    Confirm {
        /// Which action Confirm runs.
        action: ConfirmAction,
    },
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
    /// PLAN-M11 §2 phase 5: submit the human's `CommandKind::Resign` and
    /// unpause — the sim's own match rules end the match and the existing
    /// end-screen promotion takes over.
    Resign,
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
    /// Start a mission match (A-011, ADR-0201 Phase 1): bundle the mission's
    /// map, resolve its objectives through the content seam, and build the
    /// host through the engine mission seam so the sim evaluates the
    /// mission's objectives.
    StartMission {
        /// The selected mission's index into the tree's mission list.
        mission_index: usize,
        /// The seed the match will run (parsed field or fresh random).
        seed: u64,
    },
    /// Open the campaign screen (B-026, ADR-0201 §6/§9 Phase 3): the app
    /// builds the `Screen::Campaign` snapshot from
    /// `(ContentTree::campaign, CampaignProgress)` — the machine cannot
    /// (it never touches the content tree) — and enters the screen with
    /// focus on its first row. Emitted by the main menu's Campaign row,
    /// which exists only when the context says the tree carries a
    /// campaign (B-025's law: no campaign, no row).
    OpenCampaign,
    /// Record a mission as *skipped* in the campaign progress and persist
    /// it (B-026, ADR-0201 §6's second skip — distinct from completed).
    /// Emitted from the campaign screen's Skip row (no live match: the
    /// machine already updated its snapshot) and from the pause menu's
    /// confirm-gated Skip Mission rung (a live mission: the app drops the
    /// host on the way out — the successor screen is the main menu).
    SkipMission {
        /// The mission's index into the tree's mission list.
        mission_index: usize,
    },
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
    /// How many missions the loaded content tree lists (A-011: the mission
    /// picker's wrap; zero = the tree carries no missions and the Missions
    /// row stays inert).
    pub mission_count: usize,
    /// B-026 (ADR-0201 §6): whether the loaded content tree carries a
    /// campaign file. The main menu's Campaign row exists exactly when
    /// this is true — B-025's law: "the campaign-less default: the client
    /// shows no Campaign row". The row slots in after New Match, so the
    /// rows below it shift by one only when a campaign is present (the
    /// a010 Xvfb walk counts from the campaign-present layout).
    pub has_campaign: bool,
    /// B-026: the live match's mission, as a tree index, when the menu is
    /// open over a mission match (`None` on the menu, over a plain match,
    /// and after the mission state clears). The pause menu's Skip Mission
    /// row (ADR-0201 §6's second skip, confirm-gated) exists exactly when
    /// this is `Some` — appended as row 6 so every existing pause-menu
    /// index (the Xvfb scripts' focus arithmetic) keeps its meaning.
    pub live_mission: Option<usize>,
}

/// One focusable row's identity — the shared vocabulary between the
/// machine's focus model and the rendering/hit-test layer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonId {
    /// Main menu: open the New Match screen.
    NewMatch,
    /// Main menu: open the settings screen.
    Settings,
    /// Main menu / pause menu: open the Controls screen (A-009 task 1).
    Controls,
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
    /// New Match screen: open the mission picker (A-011, ADR-0201
    /// Phase 1) — the picker is a sub-screen of New Match, so the main
    /// menu's rows and every existing index stay unchanged.
    Missions,
    /// Mission picker screen: the mission field (cycles the tree's list).
    Mission,
    /// Mission picker screen: the seed field.
    MissionSeed,
    /// Mission picker screen: start the mission match.
    MissionStart,
    /// Main menu (B-026, ADR-0201 §6): open the campaign screen — row 1,
    /// between New Match and Settings, and only when the tree carries a
    /// campaign (the context's `has_campaign`; B-025's law: the
    /// campaign-less client shows no Campaign row at all).
    Campaign,
    /// Campaign screen (B-026): the mission field (cycles the campaign's
    /// entries, showing the selected entry's name and status tag).
    CampaignMission,
    /// Campaign screen: the seed field (the mission picker's law).
    CampaignSeed,
    /// Campaign screen: start the selected entry's mission match (inert
    /// on a locked entry — the unlock rule's only gate).
    CampaignStart,
    /// Campaign screen: record the selected entry as skipped (ADR-0201
    /// §6's individually-skippable law; inert unless the entry is
    /// available — a locked entry has nothing to unlock, a completed one
    /// nothing forward).
    CampaignSkip,
    /// Pause menu (B-026): skip the live mission — row 6, only over a
    /// mission match (the context's `live_mission`), confirm-gated like
    /// every match-discarding row.
    PauseSkipMission,
    /// Settings screen: one settings row (by index).
    Setting(usize),
    /// Settings screen: save and return.
    Done,
    /// Pause menu: resume the match.
    Resume,
    /// Pause menu: restart the match.
    PauseRestart,
    /// PLAN-M11 §2 phase 5: pause menu: resign the match (confirm-gated).
    PauseResign,
    /// Pause menu: quit to the main menu.
    QuitToMenu,
    /// End screen: restart (rematch).
    Rematch,
    /// End screen: back to the main menu.
    ToMenu,
    /// Confirm screen (A-009 task 2): cancel the pending action.
    ConfirmCancel,
    /// Confirm screen (A-009 task 2): run the pending action unchanged.
    ConfirmProceed,
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
            Screen::MissionPicker(_) => "mission picker screen",
            Screen::Campaign(_) => "campaign screen",
            Screen::Settings { .. } => "settings screen",
            Screen::InMatch => "match",
            Screen::PauseMenu => "pause menu",
            Screen::EndScreen => "end screen",
            Screen::Controls { .. } => "controls screen",
            Screen::Confirm { .. } => "confirm screen",
        }
    }

    /// The focused row's identity. B-026: the row list is
    /// context-dependent (the main menu's Campaign row exists only when
    /// the tree carries a campaign; the pause menu's Skip Mission row only
    /// over a live mission), so the machine resolves rows against the same
    /// [`KeyContext`] the transitions read — the renderer's row list and
    /// this one derive from the same app fields and cannot disagree.
    pub fn focused_button(&self, ctx: &KeyContext) -> Option<ButtonId> {
        self.button_at(self.focus, ctx)
    }

    /// The row identity at a focus index (None when out of range — or
    /// when the context does not carry the row that would live there).
    pub fn button_at(&self, index: usize, ctx: &KeyContext) -> Option<ButtonId> {
        Some(match (&self.screen, index) {
            (Screen::MainMenu, 0) => ButtonId::NewMatch,
            // B-026: the Campaign row slots in at index 1 when the tree
            // carries a campaign (B-025's law: no campaign, no row) — the
            // rows below shift by one only in the campaign-present
            // layout, which the repo content always is.
            (Screen::MainMenu, 1) if ctx.has_campaign => ButtonId::Campaign,
            (Screen::MainMenu, index) => match (!ctx.has_campaign, index) {
                (true, 1) | (false, 2) => ButtonId::Settings,
                (true, 2) | (false, 3) => ButtonId::Controls,
                (true, 3) | (false, 4) => ButtonId::Quit,
                _ => return None,
            },
            (Screen::NewMatch(_), 0) => ButtonId::Mode,
            (Screen::NewMatch(_), 1) => ButtonId::Seed,
            (Screen::NewMatch(_), 2) => ButtonId::Map,
            (Screen::NewMatch(_), 3) => ButtonId::Start,
            (Screen::NewMatch(_), 4) => ButtonId::Back,
            // A-011: the Missions row rides after Back (row 5) so the
            // existing rows — and every index-driven path over them, the
            // A-010 Xvfb evidence script included — stay exactly as they
            // were.
            (Screen::NewMatch(_), 5) => ButtonId::Missions,
            (Screen::MissionPicker(_), 0) => ButtonId::Mission,
            (Screen::MissionPicker(_), 1) => ButtonId::MissionSeed,
            (Screen::MissionPicker(_), 2) => ButtonId::MissionStart,
            (Screen::MissionPicker(_), 3) => ButtonId::Back,
            // B-026 (ADR-0201 §6): the campaign screen's rows — the
            // mission field, the seed, Start, Skip, Back.
            (Screen::Campaign(_), 0) => ButtonId::CampaignMission,
            (Screen::Campaign(_), 1) => ButtonId::CampaignSeed,
            (Screen::Campaign(_), 2) => ButtonId::CampaignStart,
            (Screen::Campaign(_), 3) => ButtonId::CampaignSkip,
            (Screen::Campaign(_), 4) => ButtonId::Back,
            (Screen::Settings { .. }, index) => {
                const SETTINGS_ROWS: usize = 8; // the eight keys + Done
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
            (Screen::PauseMenu, 2) => ButtonId::Controls,
            (Screen::PauseMenu, 3) => ButtonId::PauseRestart,
            (Screen::PauseMenu, 4) => ButtonId::PauseResign,
            (Screen::PauseMenu, 5) => ButtonId::QuitToMenu,
            // B-026: the Skip Mission rung — row 6, only over a mission
            // match (rows 0-5 keep their indices in both layouts; the
            // Xvfb scripts' focus arithmetic counts from there).
            (Screen::PauseMenu, 6) if ctx.live_mission.is_some() => ButtonId::PauseSkipMission,
            (Screen::EndScreen, 0) => ButtonId::Rematch,
            (Screen::EndScreen, 1) => ButtonId::ToMenu,
            (Screen::Controls { .. }, 0) => ButtonId::Back,
            // A-009 task 2: the confirm screen has two rows, Cancel first
            // (the safer default — focus 0 lands on Cancel so a mis-press
            // or an Esc cancels).
            (Screen::Confirm { .. }, 0) => ButtonId::ConfirmCancel,
            (Screen::Confirm { .. }, 1) => ButtonId::ConfirmProceed,
            _ => return None,
        })
    }

    /// How many focusable rows the current screen has (context-dependent:
    /// the Campaign and Skip Mission rows exist only when the context
    /// carries them — B-026).
    pub fn button_count(&self, ctx: &KeyContext) -> usize {
        match self.screen {
            Screen::MainMenu => 4 + usize::from(ctx.has_campaign),
            Screen::NewMatch(_) => 6, // Mode/Seed/Map/Start/Back/Missions
            Screen::MissionPicker(_) => 4, // Mission/Seed/Start/Back
            Screen::Campaign(_) => 5, // Mission/Seed/Start/Skip/Back
            Screen::Settings { .. } => 9, // 8 settings rows + Done
            Screen::InMatch => 0,
            Screen::PauseMenu => 6 + usize::from(ctx.live_mission.is_some()),
            Screen::EndScreen => 2,
            Screen::Controls { .. } => 1, // Back
            Screen::Confirm { .. } => 2,  // Cancel + Confirm
        }
    }

    /// Moves focus to an index, clamped into the screen's range (mouse
    /// hover uses this; a stale index cannot escape).
    pub fn focus_index(&mut self, index: usize, ctx: &KeyContext) {
        let count = self.button_count(ctx);
        if count > 0 {
            self.focus = index.min(count - 1);
        } else {
            self.focus = 0;
        }
    }

    /// Focus down one row (wraps — A-110).
    pub fn focus_next(&mut self, ctx: &KeyContext) {
        let count = self.button_count(ctx);
        if count > 0 {
            self.focus = (self.focus + 1) % count;
        }
    }

    /// Focus up one row (wraps — A-110).
    pub fn focus_prev(&mut self, ctx: &KeyContext) {
        let count = self.button_count(ctx);
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
            // A-011: the picker backs out to the New Match screen it was
            // opened from, restoring its fields unchanged.
            Screen::MissionPicker(ref fields) => {
                next.screen = Screen::NewMatch(fields.new_match.clone());
                (next, Effect::None)
            }
            // B-026: the campaign screen backs out to the main menu (the
            // screen the Campaign row opened it from).
            Screen::Campaign(_) => {
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
            // A-009 task 1: Esc on the Controls screen backs out to where
            // it was opened from (the same return_to pattern Settings uses).
            Screen::Controls { return_to } => {
                next.screen = match return_to {
                    SettingsReturn::MainMenu => Screen::MainMenu,
                    SettingsReturn::PauseMenu => Screen::PauseMenu,
                };
                (next, Effect::None)
            }
            // A-009 task 2: Esc on the confirm screen cancels (the default
            // focus is Cancel, so Esc and Enter-without-Down are the same
            // safe edge). Returns to the pause menu; the match is unchanged.
            Screen::Confirm { .. } => {
                next.screen = Screen::PauseMenu;
                (next, Effect::None)
            }
        }
    }

    /// Activates the row at `index` (Enter on the focused row, or a mouse
    /// click — the same core). Returns the successor state and the effect
    /// to execute.
    pub fn activate_index(&self, index: usize, ctx: &KeyContext) -> (AppState, Effect) {
        let Some(id) = self.button_at(index, ctx) else {
            return (self.clone(), Effect::None);
        };
        let mut next = self.clone();
        next.focus = index;
        let (mut next, effect) = self.activate_button(id, next, ctx);
        // A screen change starts the new screen fresh: focus on its first
        // row (entering Settings from the main menu's Settings *button*
        // must not leave focus on row 1 — the Xvfb pass caught this).
        if self.screen != next.screen {
            next.focus = 0;
        }
        (next, effect)
    }

    /// The per-button transition core ([`AppState::activate_index]'s
    /// match, extracted so the focus reset can run after it).
    fn activate_button(
        &self,
        id: ButtonId,
        mut next: AppState,
        ctx: &KeyContext,
    ) -> (AppState, Effect) {
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
            // A-009 task 1: open the Controls screen, remembering where to
            // return to (the same return_to pattern Settings uses — opened
            // from both the main menu and the pause menu).
            ButtonId::Controls => {
                next.screen = Screen::Controls {
                    return_to: match self.screen {
                        Screen::PauseMenu => SettingsReturn::PauseMenu,
                        _ => SettingsReturn::MainMenu,
                    },
                };
                (next, Effect::None)
            }
            ButtonId::Quit => (next, Effect::QuitApp),
            // B-026 (ADR-0201 §6/§9 Phase 3): the Campaign row opens the
            // campaign screen through the effect vocabulary — the machine
            // cannot build the screen's snapshot (it never touches the
            // content tree), so the app layer builds `Screen::Campaign`
            // from `(ContentTree::campaign, CampaignProgress)` on
            // `Effect::OpenCampaign` and enters it with focus 0. The row
            // only resolves when the context carries a campaign, so the
            // effect never fires campaign-less.
            ButtonId::Campaign => (next, Effect::OpenCampaign),
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
            // A-011 (ADR-0201 Phase 1, Task 1): the Missions row opens the
            // picker, carrying the New Match fields (Back restores them) and
            // the parent's seed text. Inert when the tree lists no missions —
            // there is nothing to pick, and the machine never emits a
            // StartMission the app cannot resolve.
            ButtonId::Missions => {
                if ctx.mission_count == 0 {
                    return (next, Effect::None);
                }
                if let Screen::NewMatch(fields) = &self.screen {
                    next.screen = Screen::MissionPicker(MissionPickerState {
                        new_match: fields.clone(),
                        seed_text: fields.seed_text.clone(),
                        mission_index: 0,
                        mission_count: ctx.mission_count.max(1),
                    });
                }
                (next, Effect::None)
            }
            // A-011: the picker's Mission row cycles the tree's mission list
            // (a click advances one, same as the map field).
            ButtonId::Mission => {
                if let Screen::MissionPicker(fields) = &mut next.screen {
                    fields.cycle_mission(1);
                }
                (next, Effect::None)
            }
            // A-011: the picker's seed field randomizes on Enter/click, the
            // New Match screen's Seed law verbatim.
            ButtonId::MissionSeed => {
                if let Screen::MissionPicker(fields) = &mut next.screen {
                    fields.seed_text = ctx.random_seed.to_string();
                }
                (next, Effect::None)
            }
            // A-011: Start resolves to a mission match — the effect carries
            // the selected mission's index and the seed; the app layer
            // bundles the mission's map and builds the host through the
            // engine mission seam.
            ButtonId::MissionStart => {
                let Screen::MissionPicker(fields) = &self.screen else {
                    return (next, Effect::None);
                };
                if fields.mission_count == 0 {
                    return (next, Effect::None);
                }
                let effect = Effect::StartMission {
                    mission_index: fields.mission_index,
                    seed: fields.seed_value(ctx.random_seed),
                };
                next.screen = Screen::InMatch;
                (next, effect)
            }
            // B-026: the campaign screen's Mission field cycles the
            // campaign's entries (a click advances one, same as every
            // cycling field).
            ButtonId::CampaignMission => {
                if let Screen::Campaign(fields) = &mut next.screen {
                    fields.cycle_selection(1);
                }
                (next, Effect::None)
            }
            // B-026: the campaign screen's seed field randomizes on
            // Enter/click, the New Match screen's Seed law verbatim.
            ButtonId::CampaignSeed => {
                if let Screen::Campaign(fields) = &mut next.screen {
                    fields.seed_text = ctx.random_seed.to_string();
                }
                (next, Effect::None)
            }
            // B-026: Start resolves to the selected entry's mission match
            // through the existing `Effect::StartMission` — a campaign
            // chapter or an unlocked challenge plays the exact mission
            // path the picker built (cutscene, HUD, replay identity). A
            // locked entry is inert: the unlock rule's only gate, and the
            // gate lives nowhere else (the picker stays fully open).
            ButtonId::CampaignStart => {
                let Screen::Campaign(fields) = &self.screen else {
                    return (next, Effect::None);
                };
                let selection = fields.selection;
                if !fields.startable(selection) {
                    return (next, Effect::None);
                }
                let effect = Effect::StartMission {
                    mission_index: fields.entries[selection].mission_index,
                    seed: fields.seed_value(ctx.random_seed),
                };
                next.screen = Screen::InMatch;
                (next, effect)
            }
            // B-026 (ADR-0201 §6, the individually-skippable law): Skip
            // records the selected mission as *skipped* — the machine
            // updates its snapshot (the visible row tag and the unlock
            // rule read the same records) and the effect carries the tree
            // index for the app's persist half. Inert unless the entry is
            // available: a locked entry has nothing to unlock, a completed
            // one nothing forward, a skipped one is already skipped.
            ButtonId::CampaignSkip => {
                let Screen::Campaign(fields) = &mut next.screen else {
                    return (next, Effect::None);
                };
                let selection = fields.selection;
                if !fields.skip(selection) {
                    return (next, Effect::None);
                }
                let effect = Effect::SkipMission {
                    mission_index: fields.entries[selection].mission_index,
                };
                (next, effect)
            }
            ButtonId::Back => {
                // A-009 task 1: when Back is the Controls screen's row, it
                // returns to where Controls was opened from (the same
                // return_to pattern Settings uses). On the New Match screen
                // it returns to the main menu (its prior behavior). A-011:
                // on the mission picker it restores the New Match fields the
                // picker carried (its parent screen).
                next.screen = match self.screen {
                    Screen::Controls { return_to } => match return_to {
                        SettingsReturn::MainMenu => Screen::MainMenu,
                        SettingsReturn::PauseMenu => Screen::PauseMenu,
                    },
                    Screen::MissionPicker(ref fields) => Screen::NewMatch(fields.new_match.clone()),
                    _ => Screen::MainMenu,
                };
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
            // A-009 task 2: Restart and Quit to Menu now open the confirm
            // rung before discarding a live match. The action stored is the
            // one the confirm screen runs unchanged on Proceed — the same
            // Effect::RestartMatch / Effect::DropMatch the direct path ran.
            ButtonId::PauseRestart => {
                next.screen = Screen::Confirm {
                    action: ConfirmAction::Restart,
                };
                (next, Effect::None)
            }
            ButtonId::PauseResign => {
                // PLAN-M11 §2 phase 5: Resign opens the confirm rung like
                // every destructive row (a mis-press must never end a match).
                next.screen = Screen::Confirm {
                    action: ConfirmAction::Resign,
                };
                (next, Effect::None)
            }
            ButtonId::QuitToMenu => {
                next.screen = Screen::Confirm {
                    action: ConfirmAction::QuitToMenu,
                };
                (next, Effect::None)
            }
            // B-026 (ADR-0201 §6, the pause-menu skip): row 6, only over a
            // mission match — the confirm rung like every match-discarding
            // action; the row itself only exists when the context carries
            // a live mission, so the captured index is always Some here
            // (the defensive no-op keeps the machine total).
            ButtonId::PauseSkipMission => match ctx.live_mission {
                Some(mission_index) => {
                    next.screen = Screen::Confirm {
                        action: ConfirmAction::SkipMission { mission_index },
                    };
                    (next, Effect::None)
                }
                None => (next, Effect::None),
            },
            ButtonId::Rematch => {
                next.screen = Screen::InMatch;
                (next, Effect::RestartMatch)
            }
            ButtonId::ToMenu => {
                next.screen = Screen::MainMenu;
                (next, Effect::DropMatch)
            }
            // A-009 task 2: Cancel returns to the pause menu, no effect —
            // the live match is unchanged. The match is still paused (the
            // pause menu is over a paused host).
            ButtonId::ConfirmCancel => {
                next.screen = Screen::PauseMenu;
                (next, Effect::None)
            }
            // A-009 task 2: Proceed runs the pending action unchanged —
            // RestartMatch rebuilds the same match (A15), DropMatch
            // discards it. The successor screen is the action's natural
            // destination (InMatch for Restart, MainMenu for Quit to Menu),
            // exactly what the direct path produced before the confirm rung.
            ButtonId::ConfirmProceed => {
                match self.screen {
                    Screen::Confirm { action } => match action {
                        ConfirmAction::Restart => {
                            next.screen = Screen::InMatch;
                            (next, Effect::RestartMatch)
                        }
                        ConfirmAction::QuitToMenu => {
                            next.screen = Screen::MainMenu;
                            (next, Effect::DropMatch)
                        }
                        // PLAN-M11 §2 phase 5: the successor screen is
                        // InMatch (unpaused) so the match's own end-screen
                        // promotion fires when the Resign resolves.
                        ConfirmAction::Resign => {
                            next.screen = Screen::InMatch;
                            (next, Effect::Resign)
                        }
                        // B-026: skipping the live mission leaves for the
                        // main menu — the app's SkipMission handler drops
                        // the host on the way out (the campaign screen is
                        // a fresh OpenCampaign away). The skip is recorded,
                        // the next entry unlocks, the match is gone: the
                        // same successor Quit to Menu takes.
                        ConfirmAction::SkipMission { mission_index } => {
                            next.screen = Screen::MainMenu;
                            (next, Effect::SkipMission { mission_index })
                        }
                    },
                    // Confirm* outside a Confirm screen is unreachable
                    // (the button only resolves inside one); the safe
                    // no-op keeps the machine total.
                    _ => (next, Effect::None),
                }
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
                next.focus_prev(ctx);
                (next, Effect::None)
            }
            NavKey::Down => {
                next.focus_next(ctx);
                (next, Effect::None)
            }
            NavKey::Left | NavKey::Right => {
                let dir = if matches!(key, NavKey::Right) { 1 } else { -1 };
                match next.focused_button(ctx) {
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
                    // A-011: the picker's Mission field cycles Left/Right,
                    // the map field's law verbatim.
                    Some(ButtonId::Mission) => {
                        if let Screen::MissionPicker(fields) = &mut next.screen {
                            fields.cycle_mission(dir);
                        }
                        (next, Effect::None)
                    }
                    // B-026: the campaign screen's Mission field cycles
                    // Left/Right, the same law.
                    Some(ButtonId::CampaignMission) => {
                        if let Screen::Campaign(fields) = &mut next.screen {
                            fields.cycle_selection(dir);
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
                if matches!(next.focused_button(ctx), Some(ButtonId::Seed)) {
                    if let Screen::NewMatch(fields) = &mut next.screen {
                        fields.seed_text.pop();
                    }
                }
                // A-011: the picker's seed field edits in place, same law.
                if matches!(next.focused_button(ctx), Some(ButtonId::MissionSeed)) {
                    if let Screen::MissionPicker(fields) = &mut next.screen {
                        fields.seed_text.pop();
                    }
                }
                // B-026: the campaign screen's seed field edits in place,
                // same law.
                if matches!(next.focused_button(ctx), Some(ButtonId::CampaignSeed)) {
                    if let Screen::Campaign(fields) = &mut next.screen {
                        fields.seed_text.pop();
                    }
                }
                (next, Effect::None)
            }
            NavKey::Digit(d) => {
                if matches!(next.focused_button(ctx), Some(ButtonId::Seed)) && d.is_ascii_digit() {
                    if let Screen::NewMatch(fields) = &mut next.screen {
                        if fields.seed_text.len() < 20 {
                            fields.seed_text.push(d);
                        }
                    }
                }
                // A-011: the picker's seed field types digits, same cap.
                if matches!(next.focused_button(ctx), Some(ButtonId::MissionSeed))
                    && d.is_ascii_digit()
                {
                    if let Screen::MissionPicker(fields) = &mut next.screen {
                        if fields.seed_text.len() < 20 {
                            fields.seed_text.push(d);
                        }
                    }
                }
                // B-026: the campaign screen's seed field types digits,
                // same cap.
                if matches!(next.focused_button(ctx), Some(ButtonId::CampaignSeed))
                    && d.is_ascii_digit()
                {
                    if let Screen::Campaign(fields) = &mut next.screen {
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
            mission_count: 1,
            has_campaign: false,
            live_mission: None,
        }
    }

    /// A pinned context carrying the `--seed 4242` prefill and 3 maps.
    fn ctx_prefilled() -> KeyContext {
        KeyContext {
            random_seed: 1234,
            seed_prefill: Some(4242),
            map_count: 3,
            mission_count: 1,
            has_campaign: false,
            live_mission: None,
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
        // A-009 task 1: the Controls row inserted at index 2 pushes Quit
        // to index 3 (NewMatch / Settings / Controls / Quit).
        let (_, effect) = menu().activate_index(3, &ctx());
        assert_eq!(effect, Effect::QuitApp);
    }

    #[test]
    fn main_menu_controls_opens_the_controls_screen_returning_to_the_menu() {
        // A-009 task 1: the new Controls row on the main menu.
        let (next, effect) = menu().activate_index(2, &ctx());
        assert_eq!(
            next.screen,
            Screen::Controls {
                return_to: SettingsReturn::MainMenu
            }
        );
        assert_eq!(effect, Effect::None);
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
        let (next, effect) = settings.activate_index(8, &ctx()); // Done
        assert_eq!(effect, Effect::SaveSettings);
        assert_eq!(next.screen, Screen::MainMenu);
        // From the pause menu (the awkward edge: settings must remember).
        let settings = at(Screen::Settings {
            return_to: SettingsReturn::PauseMenu,
        });
        let (next, effect) = settings.activate_index(8, &ctx());
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
    fn pause_menu_controls_opens_the_controls_screen_returning_to_the_pause_menu() {
        // A-009 task 1: the Controls row on the pause menu.
        let (next, effect) = at(Screen::PauseMenu).activate_index(2, &ctx());
        assert_eq!(
            next.screen,
            Screen::Controls {
                return_to: SettingsReturn::PauseMenu
            }
        );
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn pause_menu_restart_opens_the_confirm_screen() {
        // A-009 task 2: Restart no longer rebuilds the match directly — it
        // opens the confirm rung first (the live match will be lost).
        let (next, effect) = at(Screen::PauseMenu).activate_index(3, &ctx());
        assert_eq!(
            next.screen,
            Screen::Confirm {
                action: ConfirmAction::Restart
            }
        );
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn pause_menu_quit_to_menu_opens_the_confirm_screen() {
        // A-009 task 2: Quit to Menu no longer drops the match directly — it
        // opens the confirm rung first. (PLAN-M11 §2 phase 5 moved it to
        // row 5; Resign took row 4.)
        let (next, effect) = at(Screen::PauseMenu).activate_index(5, &ctx());
        assert_eq!(
            next.screen,
            Screen::Confirm {
                action: ConfirmAction::QuitToMenu
            }
        );
        assert_eq!(effect, Effect::None);
    }

    // ---- the Resign row (PLAN-M11 §2 phase 5) -------------------------

    #[test]
    fn pause_menu_has_six_rows_and_resign_is_row_four() {
        let pause = at(Screen::PauseMenu);
        assert_eq!(pause.button_count(&ctx()), 6);
        assert_eq!(pause.button_at(4, &ctx()), Some(ButtonId::PauseResign));
        assert_eq!(pause.button_at(5, &ctx()), Some(ButtonId::QuitToMenu));
    }

    #[test]
    fn pause_menu_resign_opens_the_confirm_screen() {
        // PLAN-M11 §2 phase 5: Resign is confirm-gated like every
        // destructive row — a mis-press must never end a match.
        let (next, effect) = at(Screen::PauseMenu).activate_index(4, &ctx());
        assert_eq!(
            next.screen,
            Screen::Confirm {
                action: ConfirmAction::Resign
            }
        );
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn confirm_proceed_on_resign_emits_the_resign_effect_and_returns_in_match() {
        // The successor screen is InMatch (unpaused) so the match's own
        // end-screen promotion fires when the Resign resolves.
        let confirm = at(Screen::Confirm {
            action: ConfirmAction::Resign,
        });
        let (next, effect) = confirm.activate_index(1, &ctx()); // Confirm
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(effect, Effect::Resign);
        // Cancel still returns to the pause menu with no effect.
        let (next, effect) = confirm.activate_index(0, &ctx());
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);
    }

    // ---- the Controls screen (A-009 task 1) ----------------------------

    #[test]
    fn controls_screen_has_one_button_the_back_row() {
        let controls = at(Screen::Controls {
            return_to: SettingsReturn::MainMenu,
        });
        assert_eq!(controls.button_count(&ctx()), 1);
        assert_eq!(controls.button_at(0, &ctx()), Some(ButtonId::Back));
        assert_eq!(controls.button_at(1, &ctx()), None);
    }

    #[test]
    fn controls_back_and_escape_return_to_the_screen_they_were_opened_from() {
        // Opened from the main menu — Back and Esc return there.
        let from_menu = at(Screen::Controls {
            return_to: SettingsReturn::MainMenu,
        });
        let (next, effect) = from_menu.activate_index(0, &ctx()); // Back
        assert_eq!(next.screen, Screen::MainMenu);
        assert_eq!(effect, Effect::None);
        let (next, effect) = from_menu.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::MainMenu);
        assert_eq!(effect, Effect::None);
        // Opened from the pause menu — Back and Esc return there (the match
        // stays paused; the player resumes through the pause menu's Resume).
        let from_pause = at(Screen::Controls {
            return_to: SettingsReturn::PauseMenu,
        });
        let (next, effect) = from_pause.activate_index(0, &ctx()); // Back
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);
        let (next, effect) = from_pause.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn controls_enter_activates_the_back_row() {
        // Keyboard nav: Up/Down is a no-op on a one-row screen (the shared
        // nav core still owns the focus), Enter activates Back, Esc backs
        // out. Default focus is row 0 (Back).
        let controls = at(Screen::Controls {
            return_to: SettingsReturn::MainMenu,
        });
        assert_eq!(controls.focus, 0);
        assert_eq!(controls.focused_button(&ctx()), Some(ButtonId::Back));
        let (next, _) = controls.on_key(NavKey::Up, &ctx());
        assert_eq!(next.focus, 0, "Up wraps on a one-row screen");
        let (next, _) = controls.on_key(NavKey::Down, &ctx());
        assert_eq!(next.focus, 0, "Down wraps on a one-row screen");
        let (next, effect) = controls.on_key(NavKey::Enter, &ctx());
        assert_eq!(next.screen, Screen::MainMenu);
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn controls_round_trip_from_pause_menu_returns_to_the_match_via_resume() {
        // Pause menu -> Controls -> Back -> Pause menu -> Resume. The whole
        // round trip preserves the match (no DropMatch, no RestartMatch).
        let paused = at(Screen::PauseMenu);
        let (controls, effect) = paused.activate_index(2, &ctx()); // Controls
        assert_eq!(effect, Effect::None);
        assert!(matches!(
            controls.screen,
            Screen::Controls {
                return_to: SettingsReturn::PauseMenu
            }
        ));
        let (back_to_pause, effect) = controls.activate_index(0, &ctx()); // Back
        assert_eq!(effect, Effect::None);
        assert_eq!(back_to_pause.screen, Screen::PauseMenu);
        let (resumed, effect) = back_to_pause.activate_index(0, &ctx()); // Resume
        assert_eq!(resumed.screen, Screen::InMatch);
        assert_eq!(effect, Effect::Unpause);
    }

    // ---- the Confirm screen (A-009 task 2) -----------------------------

    #[test]
    fn confirm_screen_has_two_rows_cancel_first_default_focus_cancel() {
        let restart = at(Screen::Confirm {
            action: ConfirmAction::Restart,
        });
        assert_eq!(restart.button_count(&ctx()), 2);
        assert_eq!(restart.button_at(0, &ctx()), Some(ButtonId::ConfirmCancel));
        assert_eq!(restart.button_at(1, &ctx()), Some(ButtonId::ConfirmProceed));
        assert_eq!(restart.button_at(2, &ctx()), None);
        // Default focus is row 0 (Cancel) — the safer mis-press.
        assert_eq!(restart.focus, 0);
        assert_eq!(
            restart.focused_button(&ctx()),
            Some(ButtonId::ConfirmCancel)
        );
    }

    #[test]
    fn confirm_cancel_and_escape_return_to_the_pause_menu_with_no_effect() {
        let restart = at(Screen::Confirm {
            action: ConfirmAction::Restart,
        });
        let (next, effect) = restart.activate_index(0, &ctx()); // Cancel
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);
        let (next, effect) = restart.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);

        let quit = at(Screen::Confirm {
            action: ConfirmAction::QuitToMenu,
        });
        let (next, effect) = quit.on_key(NavKey::Escape, &ctx());
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn confirm_default_focus_means_an_enter_alone_cancels() {
        // A mis-press (Esc ladder + Enter) lands on Cancel — the safer edge.
        let restart = at(Screen::Confirm {
            action: ConfirmAction::Restart,
        });
        let (next, effect) = restart.on_key(NavKey::Enter, &ctx());
        assert_eq!(next.screen, Screen::PauseMenu);
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn confirm_proceed_restarts_the_match_unchanged() {
        // Down to row 1 (Proceed), Enter runs RestartMatch — the same
        // effect the pause menu's Restart row ran before the confirm rung.
        let restart = at(Screen::Confirm {
            action: ConfirmAction::Restart,
        });
        let (next, _) = restart.on_key(NavKey::Down, &ctx()); // row 1
        assert_eq!(next.focused_button(&ctx()), Some(ButtonId::ConfirmProceed));
        let (next, effect) = next.on_key(NavKey::Enter, &ctx());
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(effect, Effect::RestartMatch);
    }

    #[test]
    fn confirm_proceed_drops_the_match_unchanged() {
        let quit = at(Screen::Confirm {
            action: ConfirmAction::QuitToMenu,
        });
        let (next, _) = quit.on_key(NavKey::Down, &ctx()); // row 1
        assert_eq!(next.focused_button(&ctx()), Some(ButtonId::ConfirmProceed));
        let (next, effect) = next.on_key(NavKey::Enter, &ctx());
        assert_eq!(next.screen, Screen::MainMenu);
        assert_eq!(effect, Effect::DropMatch);
    }

    #[test]
    fn confirm_round_trip_cancel_keeps_the_live_match() {
        // Pause menu -> Restart (opens Confirm) -> Cancel -> Pause menu ->
        // Resume. The whole round trip preserves the match (no DropMatch,
        // no RestartMatch).
        let paused = at(Screen::PauseMenu);
        let (confirm, effect) = paused.activate_index(3, &ctx()); // Restart
        assert_eq!(effect, Effect::None);
        assert!(matches!(
            confirm.screen,
            Screen::Confirm {
                action: ConfirmAction::Restart
            }
        ));
        let (back_to_pause, effect) = confirm.activate_index(0, &ctx()); // Cancel
        assert_eq!(effect, Effect::None);
        assert_eq!(back_to_pause.screen, Screen::PauseMenu);
        let (resumed, effect) = back_to_pause.activate_index(0, &ctx()); // Resume
        assert_eq!(resumed.screen, Screen::InMatch);
        assert_eq!(effect, Effect::Unpause);
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
        // A-009 task 1: the main menu now has four rows (NewMatch, Settings,
        // Controls, Quit). The walk visits each before wrapping.
        let mut state = menu();
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::NewMatch));
        state.focus_next(&ctx());
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::Settings));
        state.focus_next(&ctx());
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::Controls));
        state.focus_next(&ctx());
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::Quit));
        state.focus_next(&ctx()); // wraps (A-110)
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::NewMatch));
        state.focus_prev(&ctx()); // wraps back
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::Quit));
    }

    #[test]
    fn keyboard_navigation_and_activation_reach_every_button() {
        // A-009 task 1: three focus_next presses now reach Quit (was two)
        // because the Controls row sits between Settings and Quit.
        let mut state = menu();
        for _ in 0..3 {
            state.focus_next(&ctx());
        }
        assert_eq!(state.focused_button(&ctx()), Some(ButtonId::Quit));
        // Enter activates the focused row — the Quit row.
        let (_, effect) = state.on_key(NavKey::Enter, &ctx());
        assert_eq!(effect, Effect::QuitApp);
    }

    #[test]
    fn entering_a_screen_resets_the_focus_to_its_first_row() {
        // The Xvfb settings-persistence pass caught this: activating the
        // main menu's Settings button (row 1) used to carry focus 1 into
        // the screen — Enter then adjusted pan speed, not edge scroll.
        let (next, _) = menu().activate_index(1, &ctx()); // open Settings
        assert_eq!(next.focus, 0, "the entered screen starts on row 0");
        // Cycling a field on the same screen keeps the focus where it is.
        let mut new_match = new_match_screen(&ctx());
        new_match.focus_index(0, &ctx()); // Mode
        let (next, _) = new_match.activate_index(0, &ctx());
        assert_eq!(next.focus, 0, "same screen: focus stays on the row");
        assert!(matches!(next.screen, Screen::NewMatch(_)));
    }

    #[test]
    fn mouse_hover_clamps_into_the_screen_and_click_activates() {
        // A-009 task 1: a stale focus index now clamps to row 3 (Quit), not
        // row 2, because the menu grew a fourth row.
        let mut state = menu();
        state.focus_index(99, &ctx()); // a stale index cannot escape
        assert_eq!(state.focus, 3);
        state.focus_index(1, &ctx());
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
        assert_eq!(state.button_count(&ctx()), 0);
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
            s.focus_index(0, &ctx());
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
        map_row.focus_index(2, &ctx());
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
        seed_row.focus_index(1, &ctx());
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
        seed_row.focus_index(1, &ctx());
        let mut next = seed_row.clone();
        for _ in 0..30 {
            let (n, _) = next.on_key(NavKey::Digit('7'), &ctx());
            next = n;
        }
        assert!(matches!(&next.screen, Screen::NewMatch(f) if f.seed_text.len() == 20));
        // A digit with the Mode row focused types nothing.
        let mut mode_row = start.clone();
        mode_row.focus_index(0, &ctx());
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
        settings.focus_index(2, &ctx()); // zoom_min
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
        done.focus_index(8, &ctx());
        let (_, effect) = done.on_key(NavKey::Left, &ctx());
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn the_settings_screen_lists_eight_rows_plus_done() {
        let settings = at(Screen::Settings {
            return_to: SettingsReturn::MainMenu,
        });
        assert_eq!(settings.button_count(&ctx()), 9);
        for row in 0..8 {
            assert_eq!(
                settings.button_at(row, &ctx()),
                Some(ButtonId::Setting(row)),
                "row {row}"
            );
        }
        assert_eq!(settings.button_at(8, &ctx()), Some(ButtonId::Done));
        assert_eq!(settings.button_at(9, &ctx()), None);
        // Focus walks the whole ladder (keyboard reaches the new row —
        // M10.2 Phase 4's mute row is row 5, between volume and
        // fullscreen).
        let mut walking = settings.clone();
        for expected in 0..9 {
            assert_eq!(walking.focus, expected);
            walking.focus_next(&ctx());
        }
        assert_eq!(walking.focus, 0, "wraps after Done");
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

    // ---- the auto-pause decision (A-009 task 3) ------------------------

    #[test]
    fn auto_pause_fires_on_focus_loss_during_a_live_human_match() {
        // InMatch + human_orders() + not paused + focus loss => Pause.
        for mode in [MatchMode::PlayerVsAi, MatchMode::Sandbox] {
            assert_eq!(
                auto_pause_on_focus(&Screen::InMatch, mode, false, FocusEvent::Unfocused),
                Effect::Pause,
                "mode {mode:?} should auto-pause on focus loss"
            );
        }
    }

    #[test]
    fn auto_pause_fires_on_minimize_during_a_live_human_match() {
        for mode in [MatchMode::PlayerVsAi, MatchMode::Sandbox] {
            assert_eq!(
                auto_pause_on_focus(&Screen::InMatch, mode, false, FocusEvent::Minimized),
                Effect::Pause,
                "mode {mode:?} should auto-pause on minimize"
            );
        }
    }

    #[test]
    fn auto_pause_does_not_fire_for_spectated_matches() {
        // AiVsAi spectate is not the player's to guard — the AI keeps
        // running; the player is watching, not driving.
        assert_eq!(
            auto_pause_on_focus(
                &Screen::InMatch,
                MatchMode::AiVsAi,
                false,
                FocusEvent::Unfocused
            ),
            Effect::None
        );
        assert_eq!(
            auto_pause_on_focus(
                &Screen::InMatch,
                MatchMode::AiVsAi,
                false,
                FocusEvent::Minimized
            ),
            Effect::None
        );
    }

    #[test]
    fn auto_pause_does_not_fire_when_already_paused() {
        // A P-paused match stays P-paused; the focus blip does not stack a
        // second pause on top (regaining focus later does not auto-resume,
        // so the manual pause stays the source of truth).
        for event in [FocusEvent::Unfocused, FocusEvent::Minimized] {
            assert_eq!(
                auto_pause_on_focus(&Screen::InMatch, MatchMode::PlayerVsAi, true, event),
                Effect::None,
                "event {event:?} during a manual pause is a no-op"
            );
        }
    }

    #[test]
    fn auto_pause_does_not_fire_on_menus_or_the_end_screen() {
        // Not InMatch = no live match to guard. The pause menu IS the
        // paused state; the end screen has no match; the menus have no
        // host. None auto-pause.
        let screens = [
            Screen::MainMenu,
            Screen::PauseMenu,
            Screen::EndScreen,
            Screen::Settings {
                return_to: SettingsReturn::PauseMenu,
            },
            Screen::Controls {
                return_to: SettingsReturn::PauseMenu,
            },
            Screen::Confirm {
                action: ConfirmAction::Restart,
            },
        ];
        for screen in &screens {
            for event in [FocusEvent::Unfocused, FocusEvent::Minimized] {
                assert_eq!(
                    auto_pause_on_focus(screen, MatchMode::PlayerVsAi, false, event),
                    Effect::None,
                    "screen {screen:?} event {event:?} must not auto-pause"
                );
            }
        }
    }

    #[test]
    fn regaining_focus_never_auto_resumes() {
        // The classic-RTS contract: focus loss pauses; focus gain does not
        // resume. The player resumes explicitly through the pause menu.
        for mode in [MatchMode::PlayerVsAi, MatchMode::AiVsAi, MatchMode::Sandbox] {
            for paused in [false, true] {
                assert_eq!(
                    auto_pause_on_focus(&Screen::InMatch, mode, paused, FocusEvent::Focused),
                    Effect::None,
                    "mode {mode:?} paused {paused} must not auto-resume on focus gain"
                );
            }
        }
    }

    #[test]
    fn auto_pause_is_pure_over_its_inputs() {
        // Same inputs => same output (the decision is a pure function —
        // the wiring layer relies on this; calling it twice in the same
        // frame must not double-fire).
        let once = auto_pause_on_focus(
            &Screen::InMatch,
            MatchMode::PlayerVsAi,
            false,
            FocusEvent::Unfocused,
        );
        let twice = auto_pause_on_focus(
            &Screen::InMatch,
            MatchMode::PlayerVsAi,
            false,
            FocusEvent::Unfocused,
        );
        assert_eq!(once, twice);
        assert_eq!(once, Effect::Pause);
    }

    // ---- the mission picker (A-011, ADR-0201 Phase 1 Task 1) -----------

    /// A context over a three-mission tree (the cycle tests' wrap).
    fn ctx_missions() -> KeyContext {
        KeyContext {
            random_seed: 1234,
            seed_prefill: None,
            map_count: 1,
            mission_count: 3,
            has_campaign: false,
            live_mission: None,
        }
    }

    /// The mission picker a context produces (the activation path's builder:
    /// New Match -> the Missions row).
    fn picker_screen(ctx: &KeyContext) -> AppState {
        new_match_screen(ctx).activate_index(5, ctx).0
    }

    #[test]
    fn the_new_match_screen_lists_the_missions_row_fifth() {
        // A-011: the Missions row rides after Back (row 5) — the existing
        // rows keep their indices (Mode/Seed/Map/Start/Back are 0-4), the
        // shape every index-driven path (the A-010 Xvfb evidence script
        // included) walks.
        let screen = new_match_screen(&ctx());
        assert_eq!(screen.button_count(&ctx()), 6);
        assert_eq!(screen.button_at(5, &ctx()), Some(ButtonId::Missions));
        assert_eq!(screen.button_at(6, &ctx()), None);
        for (index, id) in [
            ButtonId::Mode,
            ButtonId::Seed,
            ButtonId::Map,
            ButtonId::Start,
            ButtonId::Back,
            ButtonId::Missions,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(screen.button_at(index, &ctx()), Some(id), "row {index}");
        }
    }

    #[test]
    fn the_missions_row_opens_the_picker_carrying_the_new_match_fields() {
        let start = new_match_screen(&KeyContext {
            seed_prefill: Some(4242),
            ..ctx_missions()
        });
        let (next, effect) = start.activate_index(5, &ctx_missions());
        assert_eq!(effect, Effect::None);
        let Screen::MissionPicker(fields) = &next.screen else {
            panic!("the Missions row opens the picker");
        };
        // The parent's fields ride along (Back restores them) and the
        // picker's seed starts as the parent's.
        assert_eq!(
            fields.new_match,
            NewMatchState {
                mode: MatchMode::PlayerVsAi,
                seed_text: "4242".to_string(),
                map_index: 0,
                map_count: 1,
            }
        );
        assert_eq!(fields.seed_text, "4242");
        assert_eq!(fields.mission_index, 0);
        assert_eq!(fields.mission_count, 3);
        assert_eq!(next.focus, 0, "the entered screen starts on row 0");
    }

    #[test]
    fn picker_back_and_escape_restore_the_new_match_fields() {
        let start = new_match_screen(&KeyContext {
            seed_prefill: Some(4242),
            ..ctx_missions()
        });
        let picker = start.activate_index(5, &ctx_missions()).0;
        // Back (row 3).
        let (next, effect) = picker.activate_index(3, &ctx_missions());
        assert_eq!(effect, Effect::None);
        assert_eq!(
            next.screen, start.screen,
            "Back restores the New Match fields unchanged"
        );
        // Esc is the same edge.
        let (next, _) = picker.on_key(NavKey::Escape, &ctx_missions());
        assert_eq!(next.screen, start.screen);
    }

    #[test]
    fn the_picker_lists_four_rows_in_order() {
        let picker = picker_screen(&ctx_missions());
        assert_eq!(picker.button_count(&ctx()), 4);
        for (index, id) in [
            ButtonId::Mission,
            ButtonId::MissionSeed,
            ButtonId::MissionStart,
            ButtonId::Back,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(picker.button_at(index, &ctx()), Some(id), "row {index}");
        }
        assert_eq!(picker.button_at(4, &ctx()), None);
        assert_eq!(picker.name(), "mission picker screen");
    }

    #[test]
    fn the_mission_row_cycles_and_wraps_by_the_loaded_count() {
        // The map picker's wrap law, verbatim, over the mission list.
        let mut picker = picker_screen(&ctx_missions());
        picker.focus_index(0, &ctx()); // the Mission row
        let (next, _) = picker.on_key(NavKey::Right, &ctx_missions());
        assert!(matches!(&next.screen, Screen::MissionPicker(f) if f.mission_index == 1));
        let (next, _) = AppState {
            screen: next.screen,
            focus: 0,
        }
        .on_key(NavKey::Right, &ctx_missions());
        assert!(matches!(&next.screen, Screen::MissionPicker(f) if f.mission_index == 2));
        let (next, _) = AppState {
            screen: next.screen,
            focus: 0,
        }
        .on_key(NavKey::Right, &ctx_missions());
        assert!(
            matches!(&next.screen, Screen::MissionPicker(f) if f.mission_index == 0),
            "wrapped"
        );
        let (back, _) = AppState {
            screen: next.screen,
            focus: 0,
        }
        .on_key(NavKey::Left, &ctx_missions());
        assert!(
            matches!(&back.screen, Screen::MissionPicker(f) if f.mission_index == 2),
            "wraps back"
        );
        // A click on the Mission row advances one.
        let (next, _) = picker.activate_index(0, &ctx_missions());
        assert!(matches!(&next.screen, Screen::MissionPicker(f) if f.mission_index == 1));
        // A one-mission tree stays on its only mission.
        let single = picker_screen(&ctx());
        let (same, _) = single.on_key(NavKey::Right, &ctx());
        assert!(matches!(&same.screen, Screen::MissionPicker(f) if f.mission_index == 0));
    }

    #[test]
    fn picker_seed_digits_type_backspace_deletes_and_enter_randomizes() {
        let mut picker = picker_screen(&ctx_missions());
        picker.focus_index(1, &ctx()); // the seed row
        let (mut next, _) = picker.on_key(NavKey::Digit('7'), &ctx_missions());
        for d in ['7', '7'] {
            let (n, _) = next.on_key(NavKey::Digit(d), &ctx_missions());
            next = n;
        }
        assert!(matches!(&next.screen, Screen::MissionPicker(f) if f.seed_text == "777"));
        let (next, _) = next.on_key(NavKey::Backspace, &ctx_missions());
        assert!(matches!(&next.screen, Screen::MissionPicker(f) if f.seed_text == "77"));
        let (next, _) = next.on_key(NavKey::Enter, &ctx_missions()); // randomize
        assert!(matches!(&next.screen, Screen::MissionPicker(f) if f.seed_text == "1234"));
        // A digit with the Mission row focused types nothing.
        let mut mission_row = picker_screen(&ctx_missions());
        mission_row.focus_index(0, &ctx());
        let (n, _) = mission_row.on_key(NavKey::Digit('7'), &ctx_missions());
        assert!(matches!(&n.screen, Screen::MissionPicker(f) if f.seed_text.is_empty()));
    }

    #[test]
    fn picker_start_emits_start_mission_with_the_selected_index_and_seed() {
        // A typed seed rides the effect; an empty field falls back to the
        // context's fresh random seed (the New Match screen's Start law).
        let mut picker = picker_screen(&ctx_missions());
        picker.focus_index(1, &ctx());
        let (mut typed, _) = picker.on_key(NavKey::Digit('4'), &ctx_missions());
        for d in ['2', '0', '6', '9'] {
            let (n, _) = typed.on_key(NavKey::Digit(d), &ctx_missions());
            typed = n;
        }
        // Cycle the Mission row to index 1, then Start (row 2).
        let (cycled, _) = typed.activate_index(0, &ctx_missions());
        let (next, effect) = cycled.activate_index(2, &ctx_missions());
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(
            effect,
            Effect::StartMission {
                mission_index: 1,
                seed: 42069
            }
        );
        // Empty seed: the context's random seed.
        let (next, effect) = picker_screen(&ctx_missions()).activate_index(2, &ctx_missions());
        assert_eq!(next.screen, Screen::InMatch);
        assert_eq!(
            effect,
            Effect::StartMission {
                mission_index: 0,
                seed: 1234
            }
        );
    }

    #[test]
    fn a_mission_less_tree_gates_the_picker() {
        // The tree carries no missions: the Missions row is inert (the
        // machine never emits a StartMission the app cannot resolve), and
        // the picker's own Start is equally inert if it is ever reached.
        let barren = KeyContext {
            random_seed: 1234,
            seed_prefill: None,
            map_count: 1,
            mission_count: 0,
            has_campaign: false,
            live_mission: None,
        };
        let (next, effect) = new_match_screen(&barren).activate_index(5, &barren);
        assert_eq!(effect, Effect::None);
        assert!(
            matches!(next.screen, Screen::NewMatch(_)),
            "the row is inert"
        );
        // A hand-assembled picker over zero missions starts nothing.
        let zero = AppState {
            screen: Screen::MissionPicker(MissionPickerState {
                new_match: NewMatchState {
                    mode: MatchMode::PlayerVsAi,
                    seed_text: String::new(),
                    map_index: 0,
                    map_count: 1,
                },
                seed_text: String::new(),
                mission_index: 0,
                mission_count: 0,
            }),
            focus: 2,
        };
        let (next, effect) = zero.activate_index(2, &barren);
        assert_eq!(effect, Effect::None);
        assert!(
            matches!(next.screen, Screen::MissionPicker(_)),
            "Start is inert"
        );
    }
    // ---- B-026: the campaign screen (ADR-0201 §6/§9 Phase 3) -----------

    /// A context whose tree carries a campaign (the repo content's shape —
    /// the Campaign row exists on the main menu).
    fn ctx_campaign() -> KeyContext {
        KeyContext {
            random_seed: 1234,
            seed_prefill: None,
            map_count: 1,
            mission_count: 4,
            has_campaign: true,
            live_mission: None,
        }
    }

    /// The pilot campaign's shape as a machine fixture: three chapters in
    /// play order plus one challenge extending chapter 0, each entry naming
    /// its tree mission index (the real campaign's shape — the wiring tests
    /// pin the app-built version against the real content; this fixture
    /// pins the machine's laws without touching the content tree).
    fn campaign_fixture() -> CampaignState {
        let entry = |mission_index: usize, name: &str, challenge: bool, extends: Option<usize>| {
            CampaignEntry {
                mission_index,
                name: name.to_string(),
                challenge,
                extends,
                record: None,
            }
        };
        CampaignState {
            campaign_name: "Alpha Campaign".to_string(),
            seed_text: String::new(),
            selection: 0,
            entries: vec![
                entry(0, "Crossroads", false, None),
                entry(1, "Hold the Line", false, None),
                entry(2, "Supply Run", false, None),
                entry(3, "Worker Rush", true, Some(0)),
            ],
        }
    }

    #[test]
    fn the_main_menu_gains_a_campaign_row_only_when_the_tree_carries_one() {
        // B-025's law, made structural: no campaign, no Campaign row (the
        // campaign-less layout is exactly today's four rows); a campaign
        // slots the row in at index 1 and shifts the rest down by one.
        let menu = menu();
        assert_eq!(menu.button_count(&ctx()), 4);
        assert_eq!(menu.button_at(1, &ctx()), Some(ButtonId::Settings));
        assert_eq!(menu.button_count(&ctx_campaign()), 5);
        assert_eq!(menu.button_at(1, &ctx_campaign()), Some(ButtonId::Campaign));
        assert_eq!(menu.button_at(2, &ctx_campaign()), Some(ButtonId::Settings));
        assert_eq!(menu.button_at(3, &ctx_campaign()), Some(ButtonId::Controls));
        assert_eq!(menu.button_at(4, &ctx_campaign()), Some(ButtonId::Quit));
        // The campaign-less layout has no row 4; the campaign-present one
        // has no row 5.
        assert_eq!(menu.button_at(4, &ctx()), None);
        assert_eq!(menu.button_at(5, &ctx_campaign()), None);
    }

    #[test]
    fn the_campaign_row_opens_the_screen_through_the_open_campaign_effect() {
        // The machine cannot build the screen's snapshot (it never touches
        // the content tree), so the row emits the effect and the app builds
        // `Screen::Campaign` — the state stays on the menu the machine
        // returned, and the app's interpreter swaps the screen in.
        let (next, effect) = menu().activate_index(1, &ctx_campaign());
        assert!(matches!(next.screen, Screen::MainMenu));
        assert_eq!(effect, Effect::OpenCampaign);
        // Campaign-less, the row does not exist: row 1 is Settings and no
        // OpenCampaign can ever fire.
        let (next, effect) = menu().activate_index(1, &ctx());
        assert!(matches!(next.screen, Screen::Settings { .. }));
        assert_eq!(effect, Effect::None);
    }

    #[test]
    fn the_unlock_rule_gates_chapters_on_their_predecessor() {
        // ADR-0201 §6: sequential by default — the first chapter is
        // available, every later one locked until its predecessor is
        // completed OR skipped; the challenge beside chapter 0 unlocks on
        // chapter 0's completion only.
        let state = campaign_fixture();
        assert_eq!(state.status(0), CampaignStatus::Available);
        assert_eq!(state.status(1), CampaignStatus::Locked);
        assert_eq!(state.status(2), CampaignStatus::Locked);
        assert_eq!(state.status(3), CampaignStatus::Locked);
        // Completing chapter 0 unlocks chapter 1 and the challenge...
        let mut done = campaign_fixture();
        done.entries[0].record = Some(crate::progress::MissionRecord::Completed);
        assert_eq!(done.status(1), CampaignStatus::Available);
        assert_eq!(
            done.status(3),
            CampaignStatus::Available,
            "the challenge unlocks on completion"
        );
        // ...but chapter 2 stays locked behind chapter 1.
        assert_eq!(done.status(2), CampaignStatus::Locked);
        // A record shows on its own entry.
        assert_eq!(done.status(0), CampaignStatus::Completed);
    }

    #[test]
    fn a_skip_unlocks_the_next_chapter_but_never_the_challenge() {
        // A-211: the ADR's wording and the campaign file's own comment —
        // skip is the player's way past a chapter, and the challenge's
        // gate is its chapter's COMPLETION, not its being passed.
        let mut state = campaign_fixture();
        assert!(state.skip(0));
        assert_eq!(state.status(0), CampaignStatus::Skipped);
        assert_eq!(state.status(1), CampaignStatus::Available);
        assert_eq!(state.status(2), CampaignStatus::Locked);
        assert_eq!(
            state.status(3),
            CampaignStatus::Locked,
            "a skipped chapter does not unlock its challenge"
        );
    }

    #[test]
    fn an_entry_with_its_own_record_is_startable_even_from_a_stale_file() {
        // The record itself unlocks a replay: a stale or partial progress
        // file (chapter 2 recorded, chapter 1 not — hand-editing, a
        // friend's file) degrades to "replayable", never to a soft-lock.
        let mut state = campaign_fixture();
        state.entries[2].record = Some(crate::progress::MissionRecord::Skipped);
        assert_eq!(state.status(2), CampaignStatus::Skipped);
        assert!(state.startable(2));
        // And chapter 3 follows chapter 2's record (the chain continues
        // from what IS recorded).
        assert_eq!(
            state.status(3),
            CampaignStatus::Locked,
            "the challenge still waits on chapter 0"
        );
    }

    #[test]
    fn skip_is_inert_on_locked_completed_and_already_skipped_entries() {
        // The affordance exists to unlock forward: a locked entry has
        // nothing to unlock, a completed one nothing forward, a skipped
        // one is already skipped.
        let mut state = campaign_fixture();
        assert!(!state.skip(1), "locked");
        state.entries[0].record = Some(crate::progress::MissionRecord::Completed);
        assert!(!state.skip(0), "completed");
        state.entries[0].record = Some(crate::progress::MissionRecord::Skipped);
        assert!(!state.skip(0), "already skipped");
        // An available one skips.
        state.entries[1].record = None;
        state.entries[0].record = Some(crate::progress::MissionRecord::Completed);
        assert!(state.skip(1));
    }

    #[test]
    fn campaign_start_resolves_to_the_selected_mission_and_is_inert_on_locked() {
        // Start plays the exact mission path the picker built (the effect
        // carries the entry's tree mission index); a locked entry is the
        // unlock rule's only gate.
        let mut state = campaign_fixture();
        state.selection = 1; // a locked chapter
        let screen = at(Screen::Campaign(state.clone()));
        let (next, effect) = screen.activate_index(2, &ctx_campaign());
        assert_eq!(effect, Effect::None, "locked: Start is inert");
        assert!(matches!(next.screen, Screen::Campaign(_)));
        // Select the first chapter (available) and type a seed first
        // (the seed row is row 1 — the digit lands only when focused).
        state.selection = 0;
        let mut screen = at(Screen::Campaign(state));
        screen.focus_index(1, &ctx_campaign());
        let (typed, _) = screen.on_key(NavKey::Digit('7'), &ctx_campaign());
        let (next, effect) = typed.activate_index(2, &ctx_campaign());
        assert!(matches!(next.screen, Screen::InMatch));
        assert_eq!(
            effect,
            Effect::StartMission {
                mission_index: 0,
                seed: 7
            }
        );
    }

    #[test]
    fn campaign_skip_updates_the_snapshot_and_emits_the_effect() {
        // The machine's half (the snapshot the row tag and the unlock rule
        // read) and the effect's half (the tree index the app persists)
        // run on the same activation.
        let screen = at(Screen::Campaign(campaign_fixture()));
        let (next, effect) = screen.activate_index(3, &ctx_campaign());
        assert_eq!(effect, Effect::SkipMission { mission_index: 0 });
        let Screen::Campaign(fields) = next.screen else {
            panic!("the campaign screen stays");
        };
        assert_eq!(fields.status(0), CampaignStatus::Skipped);
        assert_eq!(fields.status(1), CampaignStatus::Available);
    }

    #[test]
    fn the_campaign_screen_follows_the_picker_laws() {
        // Back/Esc return to the main menu; the mission field cycles with
        // wrap; the seed field edits in place (digits + backspace + the
        // Enter randomize).
        let screen = at(Screen::Campaign(campaign_fixture()));
        let (next, _) = screen.on_key(NavKey::Escape, &ctx_campaign());
        assert!(matches!(next.screen, Screen::MainMenu));
        // The Back row (4) does the same.
        let (next, _) = screen.activate_index(4, &ctx_campaign());
        assert!(matches!(next.screen, Screen::MainMenu));
        // The mission field cycles with wrap (4 entries).
        let (next, _) = screen.on_key(NavKey::Right, &ctx_campaign());
        let Screen::Campaign(fields) = &next.screen else {
            panic!("stays on the campaign screen");
        };
        assert_eq!(fields.selection, 1);
        let mut wrapping = at(Screen::Campaign(campaign_fixture()));
        wrapping.focus_index(0, &ctx_campaign());
        for _ in 0..4 {
            let (next, _) = wrapping.on_key(NavKey::Right, &ctx_campaign());
            wrapping = next;
        }
        let Screen::Campaign(fields) = &wrapping.screen else {
            panic!("stays on the campaign screen");
        };
        assert_eq!(fields.selection, 0, "four rights wrap to the first entry");
        // The seed field: focus row 1, type, backspace.
        let mut typing = at(Screen::Campaign(campaign_fixture()));
        typing.focus_index(1, &ctx_campaign());
        let (next, _) = typing.on_key(NavKey::Digit('4'), &ctx_campaign());
        let (next, _) = next.on_key(NavKey::Digit('2'), &ctx_campaign());
        let Screen::Campaign(fields) = &next.screen else {
            panic!("stays on the campaign screen");
        };
        assert_eq!(fields.seed_text, "42");
        assert_eq!(fields.seed_value(999), 42);
        let (next, _) = next.on_key(NavKey::Backspace, &ctx_campaign());
        let Screen::Campaign(fields) = &next.screen else {
            panic!("stays on the campaign screen");
        };
        assert_eq!(fields.seed_text, "4");
        // Enter on the seed field randomizes from the context.
        let (next, _) = next.on_key(NavKey::Enter, &ctx_campaign());
        let Screen::Campaign(fields) = &next.screen else {
            panic!("stays on the campaign screen");
        };
        assert_eq!(fields.seed_text, "1234");
    }

    #[test]
    fn the_pause_menu_gains_a_skip_mission_row_only_over_a_mission() {
        // Row 6, appended after Quit to Menu so rows 0-5 (the Xvfb focus
        // arithmetic) keep their meaning; a plain match's pause menu is
        // exactly the six rows it was.
        let pause = at(Screen::PauseMenu);
        let ctx_live = KeyContext {
            live_mission: Some(0),
            ..ctx_campaign()
        };
        assert_eq!(pause.button_count(&ctx_live), 7);
        assert_eq!(
            pause.button_at(6, &ctx_live),
            Some(ButtonId::PauseSkipMission)
        );
        assert_eq!(pause.button_count(&ctx()), 6, "no live mission, no row");
        assert_eq!(pause.button_at(6, &ctx()), None);
    }

    #[test]
    fn the_pause_skip_rung_confirms_to_the_menu_with_the_effect() {
        // A match-discarding action is confirm-gated (A-009's law); the
        // rung stores the live mission's tree index and Proceed runs it
        // unchanged — the successor screen is the main menu and the app's
        // handler drops the host on the way out.
        let ctx_live = KeyContext {
            live_mission: Some(2),
            ..ctx_campaign()
        };
        let pause = at(Screen::PauseMenu);
        let (confirm, effect) = pause.activate_index(6, &ctx_live);
        assert_eq!(effect, Effect::None);
        assert_eq!(
            confirm.screen,
            Screen::Confirm {
                action: ConfirmAction::SkipMission { mission_index: 2 }
            }
        );
        // Esc on the rung cancels back to the pause menu (the safe edge).
        let (back, effect) = confirm.on_key(NavKey::Escape, &ctx_live);
        assert!(matches!(back.screen, Screen::PauseMenu));
        assert_eq!(effect, Effect::None);
        // Proceed leaves for the main menu carrying the skip.
        let (next, effect) = confirm.activate_index(1, &ctx_live);
        assert!(matches!(next.screen, Screen::MainMenu));
        assert_eq!(effect, Effect::SkipMission { mission_index: 2 });
    }
}
