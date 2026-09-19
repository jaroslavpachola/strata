//! A query's answer as a board: a column per choice of one property, a
//! card per item, and arrows that move a card to the next column by
//! writing that property.

use egui::{Frame, Ui};
use serde_json::Value;
use strata_core::{Api, Entry, Query, TypeDef, Uuid, Values, value_text};

use crate::Loaded;

pub struct KanbanView {
    pub query: Query,
    /// The property whose choices are the columns [default: the type's
    /// first property with choices].
    pub column_by: Option<String>,
    loaded: Option<Loaded>,
    error: Option<String>,
}

impl KanbanView {
    pub fn new(query: Query) -> Self {
        Self {
            query,
            column_by: None,
            loaded: None,
            error: None,
        }
    }

    pub fn invalidate(&mut self) {
        self.loaded = None;
    }

    /// The property the board groups by, if the type has one.
    pub fn grouping(&self, def: &TypeDef) -> Option<(String, Vec<String>)> {
        def.properties
            .iter()
            .find(|p| match &self.column_by {
                Some(name) => p.name == *name,
                None => p.choices.is_some(),
            })
            .and_then(|p| Some((p.name.clone(), p.choices.clone()?)))
    }

    /// The item a card's title was clicked on, to open.
    pub fn show(&mut self, ui: &mut Ui, store: &dyn Api, author: &str) -> Option<Uuid> {
        let loaded = self
            .loaded
            .get_or_insert_with(|| Loaded::fetch(store, &self.query));
        let entries = match &loaded.entries {
            Ok(e) => e.clone(),
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
                return None;
            }
        };
        let def = loaded.def.clone()?;
        let Some((by, mut lanes)) = self.grouping(&def) else {
            ui.label("A board needs a text property with choices.");
            return None;
        };
        let locked = entries.iter().filter(|e| e.item().is_none()).count();
        if locked > 0 {
            ui.label(format!("{locked} items in the locked vault"));
        }
        let items: Vec<_> = entries.iter().filter_map(Entry::item).collect();
        let lane_of = |item: &strata_core::Item| item.values.get(&by).map(value_text);
        if items.iter().any(|i| lane_of(i).is_none()) {
            lanes.push(String::new());
        }
        let title_of = |item: &strata_core::Item| {
            def.properties
                .iter()
                .find(|p| p.name != by && p.choices.is_none())
                .and_then(|p| item.values.get(&p.name))
                .map(value_text)
                .unwrap_or_else(|| item.id.to_string())
        };

        let mut open = None;
        let mut moves: Vec<(Uuid, Value)> = Vec::new();
        let n = lanes.len();
        ui.columns(n, |cols| {
            for (i, lane) in lanes.iter().enumerate() {
                let ui = &mut cols[i];
                let cards: Vec<_> = items
                    .iter()
                    .filter(|it| lane_of(it).unwrap_or_default() == *lane)
                    .collect();
                let name = if lane.is_empty() { "(none)" } else { lane };
                ui.strong(format!("{name} ({})", cards.len()));
                for item in cards {
                    Frame::group(ui.style()).show(ui, |ui| {
                        if ui.link(title_of(item)).clicked() {
                            open = Some(item.id);
                        }
                        ui.horizontal(|ui| {
                            if i > 0 && ui.small_button("<").clicked() {
                                moves.push((item.id, lane_value(&lanes[i - 1])));
                            }
                            if i + 1 < n && ui.small_button(">").clicked() {
                                moves.push((item.id, lane_value(&lanes[i + 1])));
                            }
                        });
                    });
                }
            }
        });
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        for (id, value) in moves {
            let mut patch = Values::new();
            patch.insert(by.clone(), value);
            match store.update_item(id, patch, author) {
                Ok(_) => self.error = None,
                Err(e) => self.error = Some(e.to_string()),
            }
            self.invalidate();
        }
        open
    }
}

fn lane_value(lane: &str) -> Value {
    if lane.is_empty() {
        Value::Null
    } else {
        Value::String(lane.to_string())
    }
}
