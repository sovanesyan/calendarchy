//! Key bindings: keys → `Action`s, per navigation mode.
//!
//! Bindings live in one table, which also drives the help overlay, and any
//! action can be rebound from config (`"keys": { "join": "J" }`). Bulgarian
//! phonetic layout keys are normalised to their Latin equivalents first, so
//! each binding is written once.

use std::collections::HashMap;
use std::sync::OnceLock;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

use crate::app::NavigationMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // Day navigation
    NextDay,
    PrevDay,
    NextWeek,
    PrevWeek,
    NextMonth,
    PrevMonth,
    EnterEvents,
    // Event navigation
    NextEvent,
    PrevEvent,
    JumpEventsForward,
    JumpEventsBack,
    ExitEvents,
    SwitchPanel,
    Join,
    Accept,
    Decline,
    Delete,
    // Shared
    Today,
    Now,
    Refresh,
    ToggleLogs,
    Search,
    Help,
    OpenGoogleWeb,
    OpenICloudWeb,
    ConnectGoogle,
    ConnectICloud,
    Setup,
    Quit,
}

/// Config names for actions, as used in `"keys": { ... }`
const ACTION_NAMES: &[(Action, &str)] = &[
    (Action::NextDay, "next_day"),
    (Action::PrevDay, "prev_day"),
    (Action::NextWeek, "next_week"),
    (Action::PrevWeek, "prev_week"),
    (Action::NextMonth, "next_month"),
    (Action::PrevMonth, "prev_month"),
    (Action::EnterEvents, "enter_events"),
    (Action::NextEvent, "next_event"),
    (Action::PrevEvent, "prev_event"),
    (Action::JumpEventsForward, "jump_events_forward"),
    (Action::JumpEventsBack, "jump_events_back"),
    (Action::ExitEvents, "exit_events"),
    (Action::SwitchPanel, "switch_panel"),
    (Action::Join, "join"),
    (Action::Accept, "accept"),
    (Action::Decline, "decline"),
    (Action::Delete, "delete"),
    (Action::Today, "today"),
    (Action::Now, "now"),
    (Action::Refresh, "refresh"),
    (Action::ToggleLogs, "toggle_logs"),
    (Action::Search, "search"),
    (Action::Help, "help"),
    (Action::OpenGoogleWeb, "open_google_web"),
    (Action::OpenICloudWeb, "open_icloud_web"),
    (Action::ConnectGoogle, "connect_google"),
    (Action::ConnectICloud, "connect_icloud"),
    (Action::Setup, "setup"),
    (Action::Quit, "quit"),
];

/// A key as bound: a key code, with or without Ctrl. Other modifiers are
/// ignored (Shift is already in the character: 'J' vs 'j').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub code: KeyCode,
    pub ctrl: bool,
}

const fn ch(c: char) -> Key {
    Key { code: KeyCode::Char(c), ctrl: false }
}

const fn ctrl(c: char) -> Key {
    Key { code: KeyCode::Char(c), ctrl: true }
}

const fn code(code: KeyCode) -> Key {
    Key { code, ctrl: false }
}

impl Key {
    /// Short label for the help overlay: "l", "^d", "←", "Enter"
    pub fn label(&self) -> String {
        let base = match self.code {
            KeyCode::Char(' ') => "Space".to_string(),
            KeyCode::Char(c) => c.to_string(),
            KeyCode::Left => "\u{2190}".to_string(),
            KeyCode::Right => "\u{2192}".to_string(),
            KeyCode::Up => "\u{2191}".to_string(),
            KeyCode::Down => "\u{2193}".to_string(),
            KeyCode::Enter => "Enter".to_string(),
            KeyCode::Esc => "Esc".to_string(),
            KeyCode::Tab => "Tab".to_string(),
            KeyCode::BackTab => "S-Tab".to_string(),
            KeyCode::Backspace => "Bksp".to_string(),
            KeyCode::Home => "Home".to_string(),
            KeyCode::End => "End".to_string(),
            KeyCode::PageUp => "PgUp".to_string(),
            KeyCode::PageDown => "PgDn".to_string(),
            KeyCode::F(n) => format!("F{}", n),
            _ => "?".to_string(),
        };
        if self.ctrl { format!("^{}", base) } else { base }
    }

    /// Parse a key from config: "J", "ctrl+d" (or "^d", "C-d"), "enter",
    /// "esc", "tab", "shift+tab", "left", "space", "f5", ...
    pub fn parse(spec: &str) -> Option<Key> {
        let spec = spec.trim();
        let lower = spec.to_ascii_lowercase();
        for prefix in ["ctrl+", "ctrl-", "c-", "^"] {
            if lower.starts_with(prefix) && spec.len() > prefix.len() {
                let rest = Key::parse(&spec[prefix.len()..])?;
                return Some(Key { ctrl: true, ..rest });
            }
        }
        let mut chars = spec.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            return Some(ch(c));
        }
        let named = match lower.as_str() {
            "enter" | "return" => KeyCode::Enter,
            "esc" | "escape" => KeyCode::Esc,
            "tab" => KeyCode::Tab,
            "backtab" | "shift+tab" | "s-tab" => KeyCode::BackTab,
            "space" => KeyCode::Char(' '),
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "backspace" => KeyCode::Backspace,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pageup" => KeyCode::PageUp,
            "pagedown" => KeyCode::PageDown,
            f if f.starts_with('f') => KeyCode::F(f[1..].parse().ok().filter(|n| (1..=12).contains(n))?),
            _ => return None,
        };
        Some(code(named))
    }
}

/// Where a binding applies. Mode-specific bindings win over shared ones, which
/// lets keys like j/k and ^d mean different things per mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Day,
    Event,
    Both,
}

impl Scope {
    fn covers(self, mode: NavigationMode) -> bool {
        matches!(
            (self, mode),
            (Scope::Both, _) | (Scope::Day, NavigationMode::Day) | (Scope::Event, NavigationMode::Event)
        )
    }
}

const DEFAULTS: &[(Scope, Action, &[Key])] = {
    use Action::*;
    use Scope::*;
    &[
        // Day mode: h/l move a day sideways, j/k a week, matching the month grid
        (Day, NextDay, &[ch('l'), code(KeyCode::Right)]),
        (Day, PrevDay, &[ch('h'), code(KeyCode::Left)]),
        (Day, NextWeek, &[ch('j'), code(KeyCode::Down)]),
        (Day, PrevWeek, &[ch('k'), code(KeyCode::Up)]),
        (Day, NextMonth, &[ctrl('d')]),
        (Day, PrevMonth, &[ctrl('u')]),
        (Day, EnterEvents, &[code(KeyCode::Enter)]),
        (Day, ConnectGoogle, &[ch('g')]),
        (Day, ConnectICloud, &[ch('i')]),
        // Event mode. Esc deliberately does NOT quit anywhere: it means "back",
        // and double-Esc from Event mode would exit the app by accident
        (Event, NextEvent, &[ch('j'), code(KeyCode::Down)]),
        (Event, PrevEvent, &[ch('k'), code(KeyCode::Up)]),
        (Event, JumpEventsForward, &[ctrl('d')]),
        (Event, JumpEventsBack, &[ctrl('u')]),
        (Event, ExitEvents, &[code(KeyCode::Esc)]),
        (Event, SwitchPanel, &[code(KeyCode::Tab), code(KeyCode::BackTab)]),
        (Event, Join, &[ch('J')]),
        (Event, Accept, &[ch('a')]),
        (Event, Decline, &[ch('d')]),
        (Event, Delete, &[ch('x')]),
        // Everywhere
        (Both, NextMonth, &[ch('L')]),
        (Both, PrevMonth, &[ch('H')]),
        (Both, Today, &[ch('t')]),
        (Both, Now, &[ch('n')]),
        (Both, Refresh, &[ch('r')]),
        (Both, ToggleLogs, &[ch('D')]),
        (Both, Search, &[ch('f')]),
        (Both, Help, &[ch('?')]),
        (Both, OpenGoogleWeb, &[ch('1')]),
        (Both, OpenICloudWeb, &[ch('2')]),
        (Both, Setup, &[ch('S')]),
        (Both, Quit, &[ch('q')]),
    ]
};

/// One key, or several, for an action in config
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeySpec {
    One(String),
    Many(Vec<String>),
}

impl KeySpec {
    fn specs(&self) -> Vec<&str> {
        match self {
            KeySpec::One(s) => vec![s.as_str()],
            KeySpec::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

struct Binding {
    scope: Scope,
    action: Action,
    keys: Vec<Key>,
    /// Set from config; checked before the defaults so a rebound key can
    /// take over one that has a default meaning
    custom: bool,
}

pub struct Keymap {
    bindings: Vec<Binding>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: DEFAULTS
                .iter()
                .map(|&(scope, action, keys)| Binding { scope, action, keys: keys.to_vec(), custom: false })
                .collect(),
        }
    }
}

impl Keymap {
    /// The defaults with config overrides applied. Rebinding an action replaces
    /// its keys wherever it applies. Returns the map plus a description of any
    /// entries that couldn't be used.
    pub fn with_overrides(overrides: &HashMap<String, KeySpec>) -> (Keymap, Vec<String>) {
        let mut map = Keymap::default();
        let mut problems = Vec::new();
        let mut names: Vec<&String> = overrides.keys().collect();
        names.sort(); // deterministic problem order
        for name in names {
            let Some(&(action, _)) = ACTION_NAMES.iter().find(|(_, n)| n == name) else {
                problems.push(format!("unknown action \"{}\"", name));
                continue;
            };
            let mut keys = Vec::new();
            for spec in overrides[name].specs() {
                match Key::parse(spec) {
                    Some(key) => keys.push(key),
                    None => problems.push(format!("can't read key \"{}\" for {}", spec, name)),
                }
            }
            if keys.is_empty() {
                continue;
            }
            for binding in map.bindings.iter_mut().filter(|b| b.action == action) {
                binding.keys = keys.clone();
                binding.custom = true;
            }
        }
        (map, problems)
    }

    pub fn action_for(&self, mode: NavigationMode, key: KeyEvent) -> Option<Action> {
        let pressed = Key {
            code: match key.code {
                KeyCode::Char(c) => KeyCode::Char(latin(c)),
                other => other,
            },
            ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
        };
        let find = |pred: &dyn Fn(&Binding) -> bool| {
            self.bindings
                .iter()
                .find(|b| b.scope.covers(mode) && b.keys.contains(&pressed) && pred(b))
                .map(|b| b.action)
        };
        find(&|b| b.custom)
            .or_else(|| find(&|b| b.scope != Scope::Both))
            .or_else(|| find(&|_| true))
    }

    /// Every key bound to an action, across modes, for the help overlay
    pub fn keys_for(&self, action: Action) -> Vec<Key> {
        let mut keys: Vec<Key> = Vec::new();
        // Shared bindings first: "H/L ^u/^d" reads better than the reverse
        let mut bindings: Vec<&Binding> = self.bindings.iter().filter(|b| b.action == action).collect();
        bindings.sort_by_key(|b| b.scope != Scope::Both);
        for key in bindings.into_iter().flat_map(|b| b.keys.iter()) {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
        keys
    }

    /// Help label for a group of related actions: their keys side by side,
    /// "h/l ←/→" for [PrevDay, NextDay], or each action's keys in turn when
    /// they don't pair up
    pub fn label_for(&self, actions: &[Action]) -> String {
        let keys: Vec<Vec<Key>> = actions.iter().map(|a| self.keys_for(*a)).collect();
        let n = keys[0].len();
        if keys.iter().all(|k| k.len() == n) {
            (0..n)
                .map(|i| keys.iter().map(|k| k[i].label()).collect::<Vec<_>>().join("/"))
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            keys.iter()
                .map(|k| k.iter().map(Key::label).collect::<Vec<_>>().join(" "))
                .collect::<Vec<_>>()
                .join(" / ")
        }
    }
}

static KEYMAP: OnceLock<Keymap> = OnceLock::new();

/// Use this keymap for the rest of the run (call once, at startup)
pub fn install(keymap: Keymap) {
    let _ = KEYMAP.set(keymap);
}

/// The keymap in use: the installed one, or the defaults
pub fn active() -> &'static Keymap {
    static DEFAULT: OnceLock<Keymap> = OnceLock::new();
    KEYMAP.get().unwrap_or_else(|| DEFAULT.get_or_init(Keymap::default))
}

/// Map a Bulgarian phonetic key to the Latin key in the same position
fn latin(c: char) -> char {
    match c {
        'й' => 'j', 'к' => 'k', 'л' => 'l', 'х' => 'h',
        'Й' => 'J', 'К' => 'K', 'Л' => 'L', 'Х' => 'H',
        'т' => 't', 'р' => 'r', 'н' => 'n', 'ф' => 'f', 'г' => 'g', 'и' => 'i',
        'я' => 'q', 'а' => 'a', 'д' => 'd', 'ь' => 'x',
        'Д' => 'D', 'С' => 'S',
        other => other,
    }
}

pub fn action_for(mode: NavigationMode, key: KeyEvent) -> Option<Action> {
    active().action_for(mode, key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    #[test]
    fn test_day_mode_bindings() {
        let day = |k| action_for(NavigationMode::Day, k);
        assert_eq!(day(key('l')), Some(NextDay));
        assert_eq!(day(key('j')), Some(NextWeek));
        assert_eq!(day(key('L')), Some(NextMonth));
        assert_eq!(day(ctrl('d')), Some(NextMonth));
        assert_eq!(day(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)), Some(EnterEvents));
        assert_eq!(day(key('x')), None, "event actions don't apply to days");
        assert_eq!(day(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)), None, "Esc never quits");
    }

    #[test]
    fn test_event_mode_bindings() {
        let ev = |k| action_for(NavigationMode::Event, k);
        assert_eq!(ev(key('j')), Some(NextEvent));
        assert_eq!(ev(key('d')), Some(Decline));
        assert_eq!(ev(ctrl('d')), Some(JumpEventsForward));
        assert_eq!(ev(key('J')), Some(Join));
        assert_eq!(ev(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)), Some(SwitchPanel));
        assert_eq!(ev(key('l')), None, "no day stepping inside a day's events");
        assert_eq!(ev(key('g')), None);
        assert_eq!(ev(key('q')), Some(Quit));
    }

    #[test]
    fn test_bulgarian_phonetic_keys() {
        assert_eq!(action_for(NavigationMode::Day, key('л')), Some(NextDay));
        assert_eq!(action_for(NavigationMode::Day, key('Л')), Some(NextMonth));
        assert_eq!(action_for(NavigationMode::Event, key('д')), Some(Decline));
        assert_eq!(action_for(NavigationMode::Event, key('ь')), Some(Delete));
        assert_eq!(action_for(NavigationMode::Event, key('Й')), Some(Join));
        assert_eq!(action_for(NavigationMode::Day, key('я')), Some(Quit));
    }

    #[test]
    fn test_every_action_has_a_config_name() {
        for &(_, action, _) in DEFAULTS {
            assert!(ACTION_NAMES.iter().any(|(a, _)| *a == action), "{action:?} has no config name");
        }
    }

    #[test]
    fn test_key_parse() {
        assert_eq!(Key::parse("J"), Some(super::ch('J')));
        assert_eq!(Key::parse("ctrl+d"), Some(super::ctrl('d')));
        assert_eq!(Key::parse("^u"), Some(super::ctrl('u')));
        assert_eq!(Key::parse("C-x"), Some(super::ctrl('x')));
        assert_eq!(Key::parse("Enter"), Some(code(KeyCode::Enter)));
        assert_eq!(Key::parse("shift+tab"), Some(code(KeyCode::BackTab)));
        assert_eq!(Key::parse("space"), Some(super::ch(' ')));
        assert_eq!(Key::parse("f5"), Some(code(KeyCode::F(5))));
        assert_eq!(Key::parse("hyper"), None);
        assert_eq!(Key::parse(""), None);
    }

    #[test]
    fn test_overrides_rebind_and_take_precedence() {
        let overrides: HashMap<String, KeySpec> = serde_json::from_str(
            r#"{ "join": "o", "delete": ["Delete", "X"], "search": "/", "bogus": "z", "quit": "ctrl+nope" }"#,
        ).unwrap();
        let (map, problems) = Keymap::with_overrides(&overrides);
        let ev = |k| map.action_for(NavigationMode::Event, k);
        assert_eq!(ev(key('o')), Some(Join));
        assert_eq!(ev(key('J')), None, "the old key is released");
        assert_eq!(ev(key('X')), Some(Delete));
        assert_eq!(ev(key('x')), None);
        assert_eq!(map.action_for(NavigationMode::Day, key('/')), Some(Search));
        // An unparsable key leaves that action's defaults alone
        assert_eq!(ev(key('q')), Some(Quit));
        assert_eq!(problems.len(), 3, "{problems:?}"); // bogus action, "Delete", "ctrl+nope"
    }

    #[test]
    fn test_rebound_key_wins_over_a_default_meaning() {
        // 'd' declines by default in Event mode; binding it to delete takes it over
        let overrides: HashMap<String, KeySpec> = serde_json::from_str(r#"{ "delete": "d" }"#).unwrap();
        let (map, _) = Keymap::with_overrides(&overrides);
        assert_eq!(map.action_for(NavigationMode::Event, key('d')), Some(Delete));
    }

    #[test]
    fn test_help_labels() {
        let map = Keymap::default();
        assert_eq!(map.label_for(&[PrevDay, NextDay]), "h/l \u{2190}/\u{2192}");
        assert_eq!(map.label_for(&[PrevMonth, NextMonth]), "H/L ^u/^d");
        assert_eq!(map.label_for(&[Join]), "J");
        assert_eq!(map.label_for(&[EnterEvents, ExitEvents]), "Enter/Esc");
    }
}
