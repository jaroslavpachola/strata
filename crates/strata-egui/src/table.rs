//! A query's answer as a table: a column per property, a click on a
//! heading to sort by it, a click on a row to select it.

use egui::{Button, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use strata_core::{Api, Entry, Query, Sort, TypeDef, Uuid, value_text};

use crate::Loaded;

/// What the user asked of the table this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableAction {
    Open(Uuid),
    New,
    Delete(Uuid),
}

pub struct TableView {
    pub query: Query,
    pub selected: Option<Uuid>,
    loaded: Option<Loaded>,
}

impl TableView {
    pub fn new(query: Query) -> Self {
        Self {
            query,
            selected: None,
            loaded: None,
        }
    }

    /// Ask the store again on the next frame: something changed.
    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    pub fn show(&mut self, ui: &mut Ui, store: &dyn Api) -> Option<TableAction> {
        let loaded = self
            .loaded
            .get_or_insert_with(|| Loaded::fetch(store, &self.query));
        let mut action = None;

        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                action = Some(TableAction::New);
            }
            let selected_item = self.selected.filter(|id| loaded.item(*id).is_some());
            if ui
                .add_enabled(selected_item.is_some(), Button::new("Open"))
                .clicked()
            {
                action = selected_item.map(TableAction::Open);
            }
            if ui
                .add_enabled(selected_item.is_some(), Button::new("Delete"))
                .clicked()
            {
                action = selected_item.map(TableAction::Delete);
            }
            if let Ok(entries) = &loaded.entries {
                ui.label(format!("{} items", entries.len()));
            }
        });
        if let Err(e) = &loaded.entries {
            ui.colored_label(ui.visuals().error_fg_color, e);
            return action;
        }
        let Some(def) = loaded.def.clone() else {
            return action;
        };
        let entries = loaded.entries.as_ref().map(Vec::as_slice).unwrap_or(&[]);

        let columns = columns(&def);
        let mut resort = None;
        let mut select = None;
        TableBuilder::new(ui)
            .id_salt(("strata-table", &def.name))
            .striped(true)
            .resizable(true)
            .columns(Column::auto().at_least(60.0).clip(true), columns.len())
            .header(22.0, |mut header| {
                for c in &columns {
                    header.col(|ui| {
                        let mark = match &self.query.sort {
                            Some(s) if s.by == *c && s.descending => " v",
                            Some(s) if s.by == *c => " ^",
                            _ => "",
                        };
                        if ui
                            .button(RichText::new(format!("{c}{mark}")).strong())
                            .clicked()
                        {
                            resort = Some(c.clone());
                        }
                    });
                }
            })
            .body(|mut body| {
                for entry in entries {
                    body.row(20.0, |mut row| {
                        let id = entry.id();
                        let chosen = self.selected == Some(id);
                        row.set_selected(chosen);
                        // any cell selects the row; a double click opens it
                        for text in cells(entry, &columns) {
                            row.col(|ui| {
                                let r = ui.selectable_label(chosen, text);
                                if r.clicked() {
                                    select = Some(id);
                                }
                                if r.double_clicked() && entry.item().is_some() {
                                    action = Some(TableAction::Open(id));
                                }
                            });
                        }
                    });
                }
            });

        if let Some(id) = select {
            self.selected = Some(id);
        }
        if let Some(by) = resort {
            let descending = matches!(&self.query.sort, Some(s) if s.by == by && !s.descending);
            self.query.sort = Some(Sort { by, descending });
            self.invalidate();
        }
        action
    }
}

/// The columns: the properties, then when the item last changed.
pub(crate) fn columns(def: &TypeDef) -> Vec<String> {
    let mut cols: Vec<String> = def.properties.iter().map(|p| p.name.clone()).collect();
    cols.push("modified".into());
    cols
}

pub(crate) fn cells(entry: &Entry, columns: &[String]) -> Vec<String> {
    match entry {
        Entry::Item(item) => columns
            .iter()
            .map(|c| match c.as_str() {
                "modified" => item
                    .modified
                    .get(..16)
                    .unwrap_or(&item.modified)
                    .replace('T', " "),
                _ => item.values.get(c).map(value_text).unwrap_or_default(),
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
