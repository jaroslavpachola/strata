//! strata-egui: the store drawn by egui. [`TableView`], [`FormView`] and
//! [`KanbanView`] each show one query or one item; [`StrataBrowser`]
//! puts them beside a type picker.
//!
//! No widget holds a store. Each takes the store as a `&dyn Api` when it
//! is drawn (a `Store` or a strata-server `Client`), so a host such as
//! Plocha hands them its own, and keeps what they show between frames
//! until told to [`invalidate`](TableView::invalidate).

mod browser;
mod form;
mod kanban;
mod table;

pub use browser::{BrowserState, StrataBrowser, ViewKind};
pub use form::{FormOutcome, FormView};
pub use kanban::KanbanView;
pub use table::{TableAction, TableView};

use strata_core::{Api, Entry, Item, Query, TypeDef, Uuid};

/// A query's answer, kept between frames: asking the store every frame
/// would be a query (or a socket round trip) sixty times a second.
struct Loaded {
    def: Option<TypeDef>,
    entries: Result<Vec<Entry>, String>,
}

impl Loaded {
    fn fetch(store: &dyn Api, query: &Query) -> Self {
        Self {
            def: store.get_type(&query.type_name).ok(),
            entries: store.query(query).map_err(|e| e.to_string()),
        }
    }

    fn item(&self, id: Uuid) -> Option<&Item> {
        self.entries
            .as_ref()
            .ok()?
            .iter()
            .filter_map(Entry::item)
            .find(|i| i.id == id)
    }
}
