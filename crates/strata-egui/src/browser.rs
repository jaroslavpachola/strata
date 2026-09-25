//! The views together: a type picker beside a table, a board or a
//! standing entry form, a form over them for one item, and the vault's
//! lock.

use egui::{TextEdit, Ui};
use serde::{Deserialize, Serialize};
use strata_core::{Api, Partition, Query, TypeDef, VaultStatus};

use crate::{FormOutcome, FormView, KanbanView, TableAction, TableView};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ViewKind {
    #[default]
    Table,
    Kanban,
    /// A form for a new item of the type, which stays up after a save:
    /// the view for logging one snapshot after another.
    Form,
}

/// What a browser shows, as data: enough to bring it back as it was.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct BrowserState {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default)]
    pub view: ViewKind,
    /// The table's query, sort included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<Query>,
}

pub struct StrataBrowser {
    author: String,
    types: Option<Vec<TypeDef>>,
    vault: Option<VaultStatus>,
    current: Option<String>,
    pub view: ViewKind,
    table: Option<TableView>,
    kanban: Option<KanbanView>,
    form: Option<FormView>,
    /// The Form view's own form, apart from `form`, which is an item
    /// opened from the table or the board.
    entry: Option<FormView>,
    passphrase: String,
    message: Option<(String, bool)>,
}

impl StrataBrowser {
    pub fn new(author: impl Into<String>) -> Self {
        Self {
            author: author.into(),
            types: None,
            vault: None,
            current: None,
            view: ViewKind::Table,
            table: None,
            kanban: None,
            form: None,
            entry: None,
            passphrase: String::new(),
            message: None,
        }
    }

    /// A browser that opens as `state` describes.
    pub fn from_state(author: impl Into<String>, state: &BrowserState) -> Self {
        let mut b = Self::new(author);
        b.view = state.view;
        if let Some(name) = &state.type_name {
            b.choose(name.clone());
            if let Some(q) = &state.query {
                b.table = Some(TableView::new(q.clone()));
            }
        }
        b
    }

    pub fn state(&self) -> BrowserState {
        BrowserState {
            type_name: self.current.clone(),
            view: self.view,
            query: self.table.as_ref().map(|t| t.query.clone()),
        }
    }

    /// The store changed: ask it again on the next frame.
    pub fn invalidate(&mut self) {
        self.types = None;
        self.vault = None;
        if let Some(t) = &mut self.table {
            t.invalidate();
        }
        if let Some(k) = &mut self.kanban {
            k.invalidate();
        }
    }

    fn choose(&mut self, name: String) {
        self.table = Some(TableView::new(Query::new(&name)));
        self.kanban = Some(KanbanView::new(Query::new(&name)));
        self.current = Some(name);
        self.form = None;
        self.entry = None;
    }

    pub fn show(&mut self, ui: &mut Ui, store: &mut dyn Api) {
        let types = match &self.types {
            Some(t) => t.clone(),
            None => match store.list_types() {
                Ok(t) => {
                    self.types = Some(t.clone());
                    t
                }
                Err(e) => {
                    self.message = Some((e.to_string(), true));
                    Vec::new()
                }
            },
        };
        let vault = *self
            .vault
            .get_or_insert_with(|| store.vault_status().unwrap_or(VaultStatus::Absent));
        if self.current.is_none()
            && let Some(first) = types.first()
        {
            self.choose(first.name.clone());
        }

        self.top_bar(ui, store, vault);
        if let Some((text, error)) = &self.message {
            if *error {
                ui.colored_label(ui.visuals().error_fg_color, text);
            } else {
                ui.label(text);
            }
        }
        ui.separator();

        let mut chosen = None;
        egui::Panel::left("strata-types")
            .resizable(false)
            .show(ui, |ui| {
                for t in &types {
                    let mut label = t.name.clone();
                    if t.partition == Partition::Vault {
                        label.push_str(if vault == VaultStatus::Unlocked {
                            " (vault)"
                        } else {
                            " (locked)"
                        });
                    }
                    if ui
                        .selectable_label(self.current.as_deref() == Some(&t.name), label)
                        .clicked()
                    {
                        chosen = Some(t.name.clone());
                    }
                }
            });
        if let Some(name) = chosen {
            self.choose(name);
            self.message = None;
        }

        egui::CentralPanel::default().show(ui, |ui| {
            let store_ref: &dyn Api = &*store;
            if let Some(form) = &mut self.form {
                match form.show(ui, store_ref, &self.author) {
                    FormOutcome::Editing => {}
                    FormOutcome::Saved(item) => {
                        self.message = Some(("saved".into(), false));
                        self.form = None;
                        if let Some(t) = &mut self.table {
                            t.selected = Some(item.id);
                        }
                        self.refresh_views();
                    }
                    FormOutcome::Cancelled => self.form = None,
                }
                return;
            }
            let opened = match self.view {
                ViewKind::Table => match self.table.as_mut().and_then(|t| t.show(ui, store_ref)) {
                    Some(TableAction::Open(id)) => Some(FormView::edit(store_ref, id)),
                    Some(TableAction::New) => {
                        self.current.as_deref().map(|n| FormView::new(store_ref, n))
                    }
                    Some(TableAction::Delete(id)) => {
                        match store_ref.delete_item(id) {
                            Ok(()) => self.message = Some(("deleted".into(), false)),
                            Err(e) => self.message = Some((e.to_string(), true)),
                        }
                        self.refresh_views();
                        None
                    }
                    None => None,
                },
                ViewKind::Kanban => self
                    .kanban
                    .as_mut()
                    .and_then(|k| k.show(ui, store_ref, &self.author))
                    .map(|id| FormView::edit(store_ref, id)),
                ViewKind::Form => {
                    self.show_entry(ui, store_ref);
                    None
                }
            };
            match opened {
                Some(Ok(form)) => self.form = Some(form),
                Some(Err(e)) => self.message = Some((e.to_string(), true)),
                None => {}
            }
        });
    }

    fn show_entry(&mut self, ui: &mut Ui, store: &dyn Api) {
        let Some(name) = &self.current else {
            return;
        };
        if self.entry.is_none() {
            match FormView::new(store, name) {
                Ok(form) => self.entry = Some(form),
                Err(e) => {
                    ui.colored_label(ui.visuals().error_fg_color, e.to_string());
                    return;
                }
            }
        }
        let Some(entry) = &mut self.entry else {
            return;
        };
        match entry.show(ui, store, &self.author) {
            FormOutcome::Editing => {}
            FormOutcome::Saved(_) => {
                self.message = Some(("saved".into(), false));
                // a fresh form for the next one
                self.entry = None;
                self.refresh_views();
            }
            // Cancel on a form that stays up means start over
            FormOutcome::Cancelled => self.entry = None,
        }
    }

    fn refresh_views(&mut self) {
        if let Some(t) = &mut self.table {
            t.invalidate();
        }
        if let Some(k) = &mut self.kanban {
            k.invalidate();
        }
    }

    fn top_bar(&mut self, ui: &mut Ui, store: &mut dyn Api, vault: VaultStatus) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.view, ViewKind::Table, "Table");
            ui.selectable_value(&mut self.view, ViewKind::Kanban, "Kanban");
            ui.selectable_value(&mut self.view, ViewKind::Form, "Form");
            ui.separator();
            match vault {
                VaultStatus::Absent => {
                    ui.label("no vault");
                }
                VaultStatus::Unlocked => {
                    ui.label("vault unlocked");
                    if ui.button("Lock").clicked() {
                        let r = store.vault_lock();
                        self.after_vault(r, "vault locked");
                    }
                }
                VaultStatus::Locked => {
                    let label = ui.label("vault locked, passphrase:");
                    let field = ui
                        .add(
                            TextEdit::singleline(&mut self.passphrase)
                                .password(true)
                                .desired_width(160.0),
                        )
                        .labelled_by(label.id);
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if ui.button("Unlock").clicked() || enter {
                        let passphrase = std::mem::take(&mut self.passphrase);
                        let r = store.vault_unlock(&passphrase);
                        self.after_vault(r, "vault unlocked");
                    }
                }
            }
            if ui.button("Reload").clicked() {
                self.message = None;
                self.invalidate();
            }
        });
    }

    fn after_vault(&mut self, result: strata_core::Result<()>, done: &str) {
        self.message = Some(match result {
            Ok(()) => (done.to_string(), false),
            Err(e) => (e.to_string(), true),
        });
        self.form = None;
        self.invalidate();
    }
}
