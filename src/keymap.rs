//! Key bindings: keys → `Action`s, per navigation mode.
//!
//! Bulgarian phonetic layout keys are normalised to their Latin equivalents
//! first, so each binding is written once.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

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
    use Action::*;
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let code = match key.code {
        KeyCode::Char(c) => KeyCode::Char(latin(c)),
        other => other,
    };

    // Ctrl chords differ by mode: months in Day mode, 10-event jumps in Event mode
    if ctrl {
        return match (mode, code) {
            (NavigationMode::Day, KeyCode::Char('d')) => Some(NextMonth),
            (NavigationMode::Day, KeyCode::Char('u')) => Some(PrevMonth),
            (NavigationMode::Event, KeyCode::Char('d')) => Some(JumpEventsForward),
            (NavigationMode::Event, KeyCode::Char('u')) => Some(JumpEventsBack),
            _ => None,
        };
    }

    let mode_specific = match mode {
        NavigationMode::Day => match code {
            KeyCode::Char('l') | KeyCode::Right => Some(NextDay),
            KeyCode::Char('h') | KeyCode::Left => Some(PrevDay),
            KeyCode::Char('j') | KeyCode::Down => Some(NextWeek),
            KeyCode::Char('k') | KeyCode::Up => Some(PrevWeek),
            KeyCode::Enter => Some(EnterEvents),
            KeyCode::Char('g') => Some(ConnectGoogle),
            KeyCode::Char('i') => Some(ConnectICloud),
            // Esc deliberately does NOT quit: it means "back" everywhere else,
            // and double-Esc from Event mode would exit the app by accident
            _ => None,
        },
        NavigationMode::Event => match code {
            KeyCode::Char('j') | KeyCode::Down => Some(NextEvent),
            KeyCode::Char('k') | KeyCode::Up => Some(PrevEvent),
            KeyCode::Char('J') => Some(Join),
            KeyCode::Char('a') => Some(Accept),
            KeyCode::Char('d') => Some(Decline),
            KeyCode::Char('x') => Some(Delete),
            KeyCode::Esc => Some(ExitEvents),
            _ => None,
        },
    };
    if mode_specific.is_some() {
        return mode_specific;
    }

    match code {
        KeyCode::Char('L') => Some(NextMonth),
        KeyCode::Char('H') => Some(PrevMonth),
        KeyCode::Char('t') => Some(Today),
        KeyCode::Char('n') => Some(Now),
        KeyCode::Char('r') => Some(Refresh),
        KeyCode::Char('D') => Some(ToggleLogs),
        KeyCode::Char('f') => Some(Search),
        KeyCode::Char('?') => Some(Help),
        KeyCode::Char('1') => Some(OpenGoogleWeb),
        KeyCode::Char('2') => Some(OpenICloudWeb),
        KeyCode::Char('S') => Some(Setup),
        KeyCode::Char('q') => Some(Quit),
        _ => None,
    }
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
}
