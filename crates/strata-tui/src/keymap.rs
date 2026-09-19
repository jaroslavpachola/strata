//! Key bindings, as rcmd writes them: `"ctrl+o" = "action"` under
//! `[keys]` in the config, over built-in defaults. Only the table view's
//! keys are bindable; typing in a form, the filter line or the
//! passphrase prompt is not.

use std::collections::{BTreeMap, HashMap};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Quit,
    Up,
    Down,
    PageUp,
    PageDown,
    Top,
    Bottom,
    NextType,
    PrevType,
    Open,
    New,
    Delete,
    Sort,
    SortReverse,
    Filter,
    Unlock,
    Lock,
    Reload,
    Help,
}

const ACTIONS: &[(&str, Action)] = &[
    ("quit", Action::Quit),
    ("up", Action::Up),
    ("down", Action::Down),
    ("page-up", Action::PageUp),
    ("page-down", Action::PageDown),
    ("top", Action::Top),
    ("bottom", Action::Bottom),
    ("next-type", Action::NextType),
    ("prev-type", Action::PrevType),
    ("open", Action::Open),
    ("new", Action::New),
    ("delete", Action::Delete),
    ("sort", Action::Sort),
    ("sort-reverse", Action::SortReverse),
    ("filter", Action::Filter),
    ("unlock", Action::Unlock),
    ("lock", Action::Lock),
    ("reload", Action::Reload),
    ("help", Action::Help),
];

const DEFAULTS: &[(&str, &str)] = &[
    ("q", "quit"),
    ("f10", "quit"),
    ("up", "up"),
    ("k", "up"),
    ("down", "down"),
    ("j", "down"),
    ("pgup", "page-up"),
    ("pgdn", "page-down"),
    ("home", "top"),
    ("end", "bottom"),
    ("tab", "next-type"),
    ("right", "next-type"),
    ("shift+tab", "prev-type"),
    ("left", "prev-type"),
    ("enter", "open"),
    ("f4", "open"),
    ("n", "new"),
    ("insert", "new"),
    ("d", "delete"),
    ("f8", "delete"),
    ("s", "sort"),
    ("S", "sort-reverse"),
    ("/", "filter"),
    ("ctrl+f", "filter"),
    ("u", "unlock"),
    ("l", "lock"),
    ("ctrl+r", "reload"),
    ("f1", "help"),
    ("?", "help"),
];

pub struct Keymap {
    map: HashMap<(KeyCode, KeyModifiers), Action>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::build(&BTreeMap::new()).0
    }
}

impl Keymap {
    /// The defaults with `custom` over them. An entry that does not parse
    /// is a warning, not a failure: the TUI still starts.
    pub fn build(custom: &BTreeMap<String, String>) -> (Self, Vec<String>) {
        let mut map = HashMap::new();
        let mut warnings = Vec::new();
        let entries = DEFAULTS
            .iter()
            .map(|(k, a)| (k.to_string(), a.to_string()))
            .chain(custom.iter().map(|(k, a)| (k.clone(), a.clone())));
        for (key, action) in entries {
            match (parse_key(&key), parse_action(&action)) {
                (Some(k), Some(a)) => {
                    map.insert(k, a);
                }
                (None, _) => warnings.push(format!("keys: {key:?} is not a key")),
                (_, None) => warnings.push(format!("keys: {action:?} is not an action")),
            }
        }
        (Self { map }, warnings)
    }

    pub fn action(&self, key: KeyEvent) -> Option<Action> {
        let mut mods = key.modifiers;
        // a shifted character arrives as itself: 'S' is the binding, not shift+s
        if let KeyCode::Char(_) = key.code {
            mods.remove(KeyModifiers::SHIFT);
        }
        let code = match key.code {
            KeyCode::BackTab => return self.map.get(&(KeyCode::Tab, KeyModifiers::SHIFT)).copied(),
            c => c,
        };
        self.map.get(&(code, mods)).copied()
    }
}

pub fn parse_action(name: &str) -> Option<Action> {
    ACTIONS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
}

/// `[ctrl+][alt+][shift+]<key>`, rcmd's spelling: a single character is
/// itself (case matters), else a name: enter, tab, esc, space, backspace,
/// insert, delete, home, end, pgup, pgdn, up, down, left, right, f1..f24.
pub fn parse_key(text: &str) -> Option<(KeyCode, KeyModifiers)> {
    let (mods_text, key) = match text.rsplit_once('+') {
        Some((m, "")) => (m.strip_suffix('+').unwrap_or(m), "+"),
        Some((m, k)) => (m, k),
        None => ("", text),
    };
    let mut mods = KeyModifiers::NONE;
    for m in mods_text.split('+').filter(|m| !m.is_empty()) {
        mods |= match m.to_ascii_lowercase().as_str() {
            "ctrl" | "control" | "c" => KeyModifiers::CONTROL,
            "alt" | "meta" | "m" => KeyModifiers::ALT,
            "shift" | "s" => KeyModifiers::SHIFT,
            _ => return None,
        };
    }
    let mut chars = key.chars();
    let code = match (chars.next(), chars.next()) {
        (Some(c), None) => {
            if mods.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                mods.remove(KeyModifiers::SHIFT);
                KeyCode::Char(c.to_ascii_uppercase())
            } else {
                KeyCode::Char(c)
            }
        }
        _ => match key.to_ascii_lowercase().as_str() {
            "enter" | "return" => KeyCode::Enter,
            "tab" => KeyCode::Tab,
            "esc" | "escape" => KeyCode::Esc,
            "space" => KeyCode::Char(' '),
            "backspace" => KeyCode::Backspace,
            "insert" | "ins" => KeyCode::Insert,
            "delete" | "del" => KeyCode::Delete,
            "home" => KeyCode::Home,
            "end" => KeyCode::End,
            "pgup" | "pageup" => KeyCode::PageUp,
            "pgdn" | "pagedown" => KeyCode::PageDown,
            "up" => KeyCode::Up,
            "down" => KeyCode::Down,
            "left" => KeyCode::Left,
            "right" => KeyCode::Right,
            f if f.starts_with('f') => KeyCode::F(f[1..].parse().ok()?),
            _ => return None,
        },
    };
    Some((code, mods))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn keys_parse_as_rcmd_writes_them() {
        assert_eq!(parse_key("f5"), Some((KeyCode::F(5), KeyModifiers::NONE)));
        assert_eq!(
            parse_key("ctrl+o"),
            Some((KeyCode::Char('o'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("shift+h"),
            Some((KeyCode::Char('H'), KeyModifiers::NONE))
        );
        assert_eq!(
            parse_key("ctrl++"),
            Some((KeyCode::Char('+'), KeyModifiers::CONTROL))
        );
        assert_eq!(
            parse_key("alt+pgdn"),
            Some((KeyCode::PageDown, KeyModifiers::ALT))
        );
        assert_eq!(parse_key("hyper+x"), None);
        assert_eq!(parse_key("nope"), None);
    }

    #[test]
    fn custom_keys_override_and_bad_ones_warn() {
        let custom = BTreeMap::from([
            ("x".to_string(), "quit".to_string()),
            ("q".to_string(), "reload".to_string()),
            ("ctrl+nope".to_string(), "quit".to_string()),
            ("z".to_string(), "fly".to_string()),
        ]);
        let (map, warnings) = Keymap::build(&custom);
        assert_eq!(warnings.len(), 2);
        let none = KeyModifiers::NONE;
        assert_eq!(
            map.action(key(KeyCode::Char('x'), none)),
            Some(Action::Quit)
        );
        assert_eq!(
            map.action(key(KeyCode::Char('q'), none)),
            Some(Action::Reload)
        );
        assert_eq!(
            map.action(key(KeyCode::Char('S'), KeyModifiers::SHIFT)),
            Some(Action::SortReverse)
        );
        assert_eq!(
            map.action(key(KeyCode::BackTab, KeyModifiers::SHIFT)),
            Some(Action::PrevType)
        );
    }
}
