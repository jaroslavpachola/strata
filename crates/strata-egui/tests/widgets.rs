//! The widgets in egui's headless harness: clicked and typed at through
//! their accessibility tree, the store checked from outside.

use std::path::Path;

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use strata_core::{Query, Store, Values};
use strata_egui::{BrowserState, StrataBrowser, ViewKind};

const PASS: &str = "egui passphrase";

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

/// A browser and the store it is drawn over, as a host would hold them.
pub struct Pane {
    browser: StrataBrowser,
    store: Store,
}

fn setup() -> (tempfile::TempDir, Harness<'static, Pane>) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.vault_create(PASS).unwrap();
    store.seed().unwrap();
    store
        .add_items(
            "Task",
            vec![
                values(json!({"title": "alpha", "status": "todo"})),
                values(json!({"title": "bravo", "status": "todo"})),
                values(json!({"title": "charlie", "status": "done"})),
            ],
            "seed",
        )
        .unwrap();
    store
        .add_item(
            "PortfolioSnapshot",
            values(json!({"taken_at": "2026-09-19", "currency": "EUR", "total": 42.5, "positions": []})),
            "seed",
        )
        .unwrap();
    store.vault_lock().unwrap();
    let pane = Pane {
        browser: StrataBrowser::new("egui"),
        store,
    };
    let harness = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(
            |ui, pane: &mut Pane| pane.browser.show(ui, &mut pane.store),
            pane,
        );
    (dir, harness)
}

fn outside(dir: &Path) -> Store {
    let mut s = Store::open(dir).unwrap();
    s.vault_unlock(PASS).unwrap();
    s
}

fn task_status(dir: &Path, title: &str) -> Option<String> {
    outside(dir)
        .query(&Query::new("Task").filter("title", title))
        .unwrap()
        .pop()
        .and_then(|e| e.into_item())
        .map(|i| i.values["status"].as_str().unwrap().to_string())
}

fn click(h: &mut Harness<'_, Pane>, label: &str) {
    h.get_by_label(label).click();
    h.run();
}

fn type_into(h: &mut Harness<'_, Pane>, label: &str, text: &str) {
    h.get_by_label(label).focus();
    h.run();
    h.get_by_label(label).type_text(text);
    h.run();
}

#[test]
fn a_task_is_browsed_and_edited() {
    let (dir, mut h) = setup();
    h.run();
    click(&mut h, "Task");
    assert!(h.query_by_label("alpha").is_some());
    assert!(h.query_by_label("3 items").is_some());

    click(&mut h, "bravo");
    click(&mut h, "Open");
    assert!(h.query_by_label("Task: edit").is_some());
    click(&mut h, "status *");
    click(&mut h, "doing");
    click(&mut h, "Save");
    assert_eq!(task_status(dir.path(), "bravo").as_deref(), Some("doing"));
    assert!(h.query_by_label("saved").is_some());
}

#[test]
fn a_task_is_created_and_a_bad_value_refused() {
    let (dir, mut h) = setup();
    h.run();
    click(&mut h, "Task");
    click(&mut h, "New");
    type_into(&mut h, "title *", "delta from egui");
    click(&mut h, "status *");
    click(&mut h, "todo");
    type_into(&mut h, "due", "someday");
    click(&mut h, "Save");
    // refused, and the form is still there to fix it
    assert!(h.query_by_label("Task: new").is_some());
    assert!(task_status(dir.path(), "delta from egui").is_none());

    h.get_by_label("due").focus();
    h.run();
    for _ in 0.."someday".len() {
        h.key_press(egui::Key::Backspace);
        h.run();
    }
    click(&mut h, "Save");
    assert_eq!(
        task_status(dir.path(), "delta from egui").as_deref(),
        Some("todo")
    );
}

#[test]
fn the_vault_is_unlocked_and_a_snapshot_edited() {
    let (dir, mut h) = setup();
    h.run();
    click(&mut h, "PortfolioSnapshot (locked)");
    assert!(
        h.query_by_label_contains("locked ").is_some(),
        "a placeholder row"
    );
    assert!(h.query_by_label("EUR").is_none());

    type_into(&mut h, "vault locked, passphrase:", "wrong");
    click(&mut h, "Unlock");
    assert!(h.query_by_label_contains("wrong passphrase").is_some());
    type_into(&mut h, "vault locked, passphrase:", PASS);
    click(&mut h, "Unlock");
    assert!(h.query_by_label("Lock").is_some());
    assert!(h.query_by_label("EUR").is_some());

    click(&mut h, "EUR");
    click(&mut h, "Open");
    // typing goes after what is there: 42.5 becomes 42.51
    type_into(&mut h, "total *", "1");
    click(&mut h, "Save");
    let snapshots = outside(dir.path())
        .query(&Query::new("PortfolioSnapshot"))
        .unwrap();
    assert_eq!(snapshots[0].item().unwrap().values["total"], 42.51);

    click(&mut h, "Lock");
    assert!(h.query_by_label("EUR").is_none());
}

#[test]
fn the_board_moves_a_card() {
    let (dir, mut h) = setup();
    h.run();
    click(&mut h, "Task");
    click(&mut h, "Kanban");
    assert!(h.query_by_label("todo (2)").is_some());
    assert!(h.query_by_label("done (1)").is_some());
    // charlie is done: its only arrow goes back to doing
    let arrows = h.get_all_by_label("<").count();
    assert_eq!(arrows, 1);
    h.get_by_label("<").click();
    h.run();
    assert_eq!(task_status(dir.path(), "charlie").as_deref(), Some("doing"));
    assert!(h.query_by_label("doing (1)").is_some());
    // a card's title opens it
    click(&mut h, "charlie");
    assert!(h.query_by_label("Task: edit").is_some());
}

#[test]
fn the_browser_comes_back_from_its_state() {
    let (_dir, mut h) = setup();
    h.run();
    click(&mut h, "Task");
    click(&mut h, "Kanban");
    let state = h.state().browser.state();
    assert_eq!(state.type_name.as_deref(), Some("Task"));
    assert_eq!(state.view, ViewKind::Kanban);
    let json = serde_json::to_value(&state).unwrap();
    let back: BrowserState = serde_json::from_value(json).unwrap();
    assert_eq!(back, state);
    h.state_mut().browser = StrataBrowser::from_state("egui", &back);
    h.run();
    assert!(h.query_by_label("todo (2)").is_some());
}
