//! The TUI's state and what keys do to it. Nothing here touches the
//! terminal: `handle_key` in, `ui::draw` out, so tests drive it directly.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::Value;
use strata_core::{
    Api, Entry, Kind, PropertyDef, Query, Sort, TypeDef, Uuid, Values, VaultStatus, value_text,
};
use strata_server::{Event, EventKind};

use crate::keymap::{Action, Keymap};
use crate::theme::Theme;

pub enum Mode {
    Browse,
    /// Typing the filter line; the text so far.
    Filter(String),
    Form(Form),
    /// Typing the vault passphrase.
    Passphrase(String),
    ConfirmDelete(Uuid),
    Help,
}

/// An item being edited, or a new one (`id: None`).
pub struct Form {
    pub id: Option<Uuid>,
    pub type_name: String,
    pub fields: Vec<Field>,
    pub focus: usize,
}

pub struct Field {
    pub prop: PropertyDef,
    pub input: String,
    /// What the input was when the form opened: only changed fields are
    /// written back.
    pub original: String,
}

pub struct App {
    store: Box<dyn Api>,
    author: String,
    pub keymap: Keymap,
    pub theme: Theme,
    pub types: Vec<TypeDef>,
    pub current: usize,
    /// The current type's items, as the store answered the query.
    pub entries: Vec<Entry>,
    /// The entries shown, after the filter line's free text.
    pub rows: Vec<usize>,
    pub selected: usize,
    pub sort: Option<Sort>,
    /// The applied filter line.
    pub filter: String,
    pub mode: Mode,
    /// A message for the status line, and whether it is an error.
    pub message: Option<(String, bool)>,
    pub vault: VaultStatus,
    pub quit: bool,
    /// Rows a page holds, as last drawn.
    pub page: usize,
    /// `$STRATA_VAULT_PASSPHRASE`, used instead of asking when set.
    pub env_passphrase: Option<String>,
}

impl App {
    pub fn new(store: Box<dyn Api>, author: String, keymap: Keymap, theme: Theme) -> Self {
        let mut app = Self {
            store,
            author,
            keymap,
            theme,
            types: Vec::new(),
            current: 0,
            entries: Vec::new(),
            rows: Vec::new(),
            selected: 0,
            sort: None,
            filter: String::new(),
            mode: Mode::Browse,
            message: None,
            vault: VaultStatus::Absent,
            quit: false,
            page: 10,
            env_passphrase: None,
        };
        app.reload();
        app
    }

    pub fn current_type(&self) -> Option<&TypeDef> {
        self.types.get(self.current)
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.rows.get(self.selected).map(|&i| &self.entries[i])
    }

    /// The table's columns: the properties, then when it last changed.
    pub fn columns(&self) -> Vec<String> {
        let mut cols: Vec<String> = self
            .current_type()
            .map(|t| t.properties.iter().map(|p| p.name.clone()).collect())
            .unwrap_or_default();
        cols.push("modified".into());
        cols
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), false));
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), true));
    }

    /// Types, vault state and the current type's items, again.
    pub fn reload(&mut self) {
        let name = self.current_type().map(|t| t.name.clone());
        match self.store.list_types() {
            Ok(types) => self.types = types,
            Err(e) => self.warn(e.to_string()),
        }
        self.current = name
            .and_then(|n| self.types.iter().position(|t| t.name == n))
            .unwrap_or(0);
        if let Ok(v) = self.store.vault_status() {
            self.vault = v;
        }
        self.load_entries();
    }

    fn load_entries(&mut self) {
        let keep = self.selected_entry().map(Entry::id);
        self.entries.clear();
        let Some(def) = self.current_type() else {
            self.rows.clear();
            return;
        };
        let (equal, _) = parse_filter(&self.filter);
        let mut q = Query::new(&def.name);
        q.filter = equal;
        q.sort = self.sort.clone();
        match self.store.query(&q) {
            Ok(entries) => self.entries = entries,
            Err(e) => self.warn(e.to_string()),
        }
        self.apply_text_filter();
        if let Some(id) = keep
            && let Some(pos) = self.rows.iter().position(|&i| self.entries[i].id() == id)
        {
            self.selected = pos;
        }
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
    }

    fn apply_text_filter(&mut self) {
        let (_, words) = parse_filter(&self.filter);
        let words: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
        let columns = self.columns();
        self.rows = (0..self.entries.len())
            .filter(|&i| {
                if words.is_empty() {
                    return true;
                }
                let text = row_cells(&self.entries[i], &columns)
                    .join(" ")
                    .to_lowercase();
                words.iter().all(|w| text.contains(w.as_str()))
            })
            .collect();
    }

    /// A change the server reported: reload if it touches what is shown.
    pub fn on_event(&mut self, event: &Event) {
        let shown = self.current_type().map(|t| t.name.as_str());
        match event.kind {
            EventKind::Vault | EventKind::Type => self.reload(),
            EventKind::Items if event.concerns(shown) => self.load_entries(),
            EventKind::Items => {}
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::Browse => self.browse_key(key),
            Mode::Help => {}
            Mode::Filter(text) => self.filter_key(text, key),
            Mode::Passphrase(text) => self.passphrase_key(text, key),
            Mode::ConfirmDelete(id) => {
                if key.code == KeyCode::Char('y') {
                    match self.store.delete_item(id) {
                        Ok(()) => self.info("deleted"),
                        Err(e) => self.warn(e.to_string()),
                    }
                    self.load_entries();
                }
            }
            Mode::Form(form) => self.form_key(form, key),
        }
    }

    fn browse_key(&mut self, key: KeyEvent) {
        let Some(action) = self.keymap.action(key) else {
            return;
        };
        let last = self.rows.len().saturating_sub(1);
        match action {
            Action::Quit => self.quit = true,
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => self.selected = (self.selected + 1).min(last),
            Action::PageUp => self.selected = self.selected.saturating_sub(self.page),
            Action::PageDown => self.selected = (self.selected + self.page).min(last),
            Action::Top => self.selected = 0,
            Action::Bottom => self.selected = last,
            Action::NextType | Action::PrevType => {
                if !self.types.is_empty() {
                    let n = self.types.len();
                    self.current = match action {
                        Action::NextType => (self.current + 1) % n,
                        _ => (self.current + n - 1) % n,
                    };
                    self.selected = 0;
                    self.sort = None;
                    self.filter.clear();
                    self.message = None;
                    self.load_entries();
                }
            }
            Action::Open => match self.selected_entry() {
                Some(Entry::Item(item)) => {
                    let item = item.clone();
                    if let Some(def) = self.current_type() {
                        self.mode = Mode::Form(Form::edit(def, &item));
                    }
                }
                Some(Entry::Locked(_)) => self.warn("the vault is locked: u to unlock"),
                None => {}
            },
            Action::New => {
                if let Some(def) = self.current_type() {
                    self.mode = Mode::Form(Form::new(def));
                }
            }
            Action::Delete => match self.selected_entry() {
                Some(Entry::Item(item)) => self.mode = Mode::ConfirmDelete(item.id),
                Some(Entry::Locked(_)) => self.warn("the vault is locked: u to unlock"),
                None => {}
            },
            Action::Sort => {
                let columns = self.columns();
                let next = match &self.sort {
                    None => columns.first().cloned(),
                    Some(s) => columns
                        .iter()
                        .position(|c| *c == s.by)
                        .and_then(|i| columns.get(i + 1))
                        .cloned(),
                };
                self.sort = next.map(|by| Sort {
                    by,
                    descending: false,
                });
                self.load_entries();
            }
            Action::SortReverse => {
                let s = self.sort.take().unwrap_or(Sort {
                    by: "created".into(),
                    descending: false,
                });
                self.sort = Some(Sort {
                    descending: !s.descending,
                    ..s
                });
                self.load_entries();
            }
            Action::Filter => self.mode = Mode::Filter(self.filter.clone()),
            Action::Unlock => match self.vault {
                VaultStatus::Unlocked => self.info("the vault is unlocked"),
                VaultStatus::Absent => self.warn("there is no vault: `strata init` makes one"),
                VaultStatus::Locked => match self.env_passphrase.clone() {
                    Some(p) => self.unlock(&p),
                    None => self.mode = Mode::Passphrase(String::new()),
                },
            },
            Action::Lock => {
                match self.store.vault_lock() {
                    Ok(()) => self.info("vault locked"),
                    Err(e) => self.warn(e.to_string()),
                }
                self.reload();
            }
            Action::Reload => {
                self.message = None;
                self.reload();
            }
            Action::Help => self.mode = Mode::Help,
        }
    }

    fn filter_key(&mut self, mut text: String, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => {
                self.filter = text.trim().to_string();
                self.selected = 0;
                self.message = None;
                self.load_entries();
            }
            KeyCode::Esc => {}
            _ => {
                edit_text(&mut text, key);
                self.mode = Mode::Filter(text);
            }
        }
    }

    fn passphrase_key(&mut self, mut text: String, key: KeyEvent) {
        match key.code {
            KeyCode::Enter => self.unlock(&text),
            KeyCode::Esc => {}
            _ => {
                edit_text(&mut text, key);
                self.mode = Mode::Passphrase(text);
            }
        }
    }

    fn unlock(&mut self, passphrase: &str) {
        match self.store.vault_unlock(passphrase) {
            Ok(()) => self.info("vault unlocked"),
            Err(e) => self.warn(e.to_string()),
        }
        self.reload();
    }

    fn form_key(&mut self, mut form: Form, key: KeyEvent) {
        let last = form.fields.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return,
            KeyCode::Enter => match self.save(&form) {
                Ok(id) => {
                    self.info("saved");
                    self.load_entries();
                    if let Some(pos) = self.rows.iter().position(|&i| self.entries[i].id() == id) {
                        self.selected = pos;
                    }
                    return;
                }
                Err(e) => self.warn(e),
            },
            KeyCode::Up | KeyCode::BackTab => form.focus = form.focus.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => form.focus = (form.focus + 1).min(last),
            KeyCode::Left | KeyCode::Right => {
                if let Some(field) = form.fields.get_mut(form.focus) {
                    field.cycle(key.code == KeyCode::Right);
                }
            }
            _ => {
                if let Some(field) = form.fields.get_mut(form.focus) {
                    edit_text(&mut field.input, key);
                }
            }
        }
        self.mode = Mode::Form(form);
    }

    /// Write the form back: every field for a new item, the changed ones
    /// for an existing one. The item's id, or why not.
    fn save(&mut self, form: &Form) -> Result<Uuid, String> {
        let mut values = Values::new();
        for field in &form.fields {
            let changed = field.input != field.original;
            if form.id.is_none() && field.input.trim().is_empty() {
                continue;
            }
            if form.id.is_some() && !changed {
                continue;
            }
            let value = field
                .prop
                .kind
                .parse_text(&field.input)
                .map_err(|e| format!("{}: {e}", field.prop.name))?;
            values.insert(field.prop.name.clone(), value);
        }
        let result = match form.id {
            Some(id) => self.store.update_item(id, values, &self.author),
            None => self.store.add_item(&form.type_name, values, &self.author),
        };
        result.map(|item| item.id).map_err(|e| e.to_string())
    }
}

impl Form {
    fn new(def: &TypeDef) -> Self {
        Self {
            id: None,
            type_name: def.name.clone(),
            fields: def
                .properties
                .iter()
                .map(|p| Field {
                    prop: p.clone(),
                    input: String::new(),
                    original: String::new(),
                })
                .collect(),
            focus: 0,
        }
    }

    fn edit(def: &TypeDef, item: &strata_core::Item) -> Self {
        let mut form = Self::new(def);
        form.id = Some(item.id);
        for field in &mut form.fields {
            if let Some(v) = item.values.get(&field.prop.name) {
                field.input = cell(v);
                field.original = field.input.clone();
            }
        }
        form
    }
}

impl Field {
    /// Left and right step through a closed set: the choices, or true and
    /// false; an optional one passes through empty too.
    fn cycle(&mut self, forward: bool) {
        let mut options: Vec<String> = match (&self.prop.choices, self.prop.kind) {
            (Some(c), _) => c.clone(),
            (None, Kind::Bool) => vec!["true".into(), "false".into()],
            _ => return,
        };
        if !self.prop.required {
            options.insert(0, String::new());
        }
        let n = options.len();
        let at = options.iter().position(|o| *o == self.input);
        let next = match (at, forward) {
            (Some(i), true) => (i + 1) % n,
            (Some(i), false) => (i + n - 1) % n,
            (None, _) => 0,
        };
        self.input = options[next].clone();
    }
}

/// Typing into a line: characters, backspace, ctrl+u to clear.
fn edit_text(text: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Backspace => {
            text.pop();
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => text.clear(),
        KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => text.push(c),
        _ => {}
    }
}

/// The filter line: `property=value` words filter in the store (the value
/// is JSON if it parses, so `n=3` is a number; double quotes keep spaces);
/// the other words must appear somewhere in a row.
pub fn parse_filter(text: &str) -> (Values, Vec<String>) {
    let mut equal = Values::new();
    let mut words = Vec::new();
    for token in split_quoted(text) {
        match token.split_once('=') {
            Some((k, v)) if !k.is_empty() => {
                let value =
                    serde_json::from_str(v).unwrap_or_else(|_| Value::String(v.to_string()));
                equal.insert(k.to_string(), value);
            }
            _ => words.push(token),
        }
    }
    (equal, words)
}

/// Split on whitespace, except inside double quotes, which stay.
fn split_quoted(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for c in text.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            c if c.is_whitespace() && !quoted => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A value as a table cell.
pub fn cell(v: &Value) -> String {
    value_text(v)
}

/// An entry's cells, one per column.
pub fn row_cells(entry: &Entry, columns: &[String]) -> Vec<String> {
    match entry {
        Entry::Item(item) => columns
            .iter()
            .map(|c| match c.as_str() {
                "modified" => item
                    .modified
                    .get(..16)
                    .unwrap_or(&item.modified)
                    .replace('T', " "),
                _ => item.values.get(c).map(cell).unwrap_or_default(),
            })
            .collect(),
        Entry::Locked(l) => {
            let mut cells = vec![String::new(); columns.len()];
            if let Some(first) = cells.first_mut() {
                *first = format!("locked {}", l.id);
            }
            cells
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_line_splits_into_equality_and_words() {
        let (equal, words) = parse_filter(r#"status=todo n=3 title="two words" misc"#);
        assert_eq!(equal["status"], "todo");
        assert_eq!(equal["n"], 3);
        assert_eq!(equal["title"], "two words");
        assert_eq!(words, ["misc"]);
    }
}
