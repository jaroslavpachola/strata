//! The TUI driven by keys and read off a TestBackend screen; the store
//! checked through a second handle on the same files.

use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use strata_core::{Query, Store, Values};
use strata_tui::app::{App, Mode};
use strata_tui::keymap::Keymap;
use strata_tui::theme::Theme;
use strata_tui::ui;

const PASS: &str = "tui passphrase";

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

/// A store with the seed types, three tasks and a snapshot, the vault
/// locked; and a TUI over it.
fn setup() -> (tempfile::TempDir, App) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.vault_create(PASS).unwrap();
    store.seed().unwrap();
    store
        .add_items(
            "Task",
            vec![
                values(json!({"title": "alpha", "status": "todo"})),
                values(json!({"title": "bravo", "status": "todo", "due": "2026-10-01"})),
                values(json!({"title": "charlie", "status": "done"})),
            ],
            "seed",
        )
        .unwrap();
    store
        .add_item(
            "PortfolioSnapshot",
            values(
                json!({"taken_at": "2026-09-19T10:00:00Z", "currency": "EUR", "total": 42.5,
                          "positions": [{"symbol": "VWCE", "quantity": 1, "price": 42.5}]}),
            ),
            "seed",
        )
        .unwrap();
    store.vault_lock().unwrap();
    let app = App::new(
        Box::new(store),
        "tui".into(),
        Keymap::default(),
        Theme::mc(),
    );
    (dir, app)
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn ctrl(app: &mut App, c: char) {
    app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| ui::draw(f, app)).unwrap();
    let buffer = terminal.backend().buffer();
    (0..buffer.area.height)
        .map(|y| {
            (0..buffer.area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A second look at the store, from outside the TUI.
fn store(dir: &Path) -> Store {
    let mut s = Store::open(dir).unwrap();
    s.vault_unlock(PASS).unwrap();
    s
}

fn task(dir: &Path, title: &str) -> Option<strata_core::Item> {
    store(dir)
        .query(&Query::new("Task").filter("title", title))
        .unwrap()
        .pop()
        .and_then(|e| e.into_item())
}

/// The M7 gate, the Task half: browse, edit, create.
#[test]
fn a_task_is_browsed_edited_and_created_with_keys_alone() {
    let (dir, mut app) = setup();
    // types come in name order: the snapshot first, then Task
    let s = screen(&mut app);
    assert!(s.contains("PortfolioSnapshot (vault)"), "{s}");
    press(&mut app, KeyCode::Tab);
    let s = screen(&mut app);
    assert!(s.contains("Task · 3"), "{s}");
    for heading in ["title", "status", "due", "project", "note", "modified"] {
        assert!(s.contains(heading), "no {heading} column:\n{s}");
    }
    assert!(s.contains("alpha") && s.contains("charlie"));

    // edit bravo's status: its choices step with the arrows
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Enter);
    assert!(screen(&mut app).contains("Task: edit"));
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Enter);
    let bravo = task(dir.path(), "bravo").unwrap();
    assert_eq!(bravo.values["status"], "doing");
    assert_eq!(bravo.values["due"], "2026-10-01", "untouched fields stay");
    assert_eq!(bravo.modified_by, "tui");
    assert!(screen(&mut app).contains("doing"));

    // a new one
    press(&mut app, KeyCode::Char('n'));
    type_text(&mut app, "delta from the tui");
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Right);
    press(&mut app, KeyCode::Down);
    type_text(&mut app, "2026-12-24");
    press(&mut app, KeyCode::Enter);
    let delta = task(dir.path(), "delta from the tui").unwrap();
    assert_eq!(delta.values["status"], "todo");
    assert_eq!(delta.values["due"], "2026-12-24");
    assert_eq!(delta.author, "tui");

    // a bad value is refused in place, and the form stays open
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    ctrl(&mut app, 'u');
    type_text(&mut app, "next week");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Form(_)));
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, err)| *err && m.contains("due"))
    );
    press(&mut app, KeyCode::Esc);
    assert_eq!(
        task(dir.path(), "delta from the tui").unwrap().values["due"],
        "2026-12-24"
    );
}

/// The M7 gate, the vault half: locked rows, `u`, then browse and edit.
#[test]
fn a_snapshot_is_unlocked_browsed_edited_and_created() {
    let (dir, mut app) = setup();
    let s = screen(&mut app);
    assert!(s.contains("locked"), "{s}");
    assert!(!s.contains("EUR"), "a value through a locked vault:\n{s}");
    press(&mut app, KeyCode::Enter);
    assert!(matches!(app.mode, Mode::Browse), "no form on a locked row");

    // a wrong passphrase first, then the right one
    press(&mut app, KeyCode::Char('u'));
    type_text(&mut app, "wrong");
    assert!(screen(&mut app).contains("Passphrase: *****_"));
    press(&mut app, KeyCode::Enter);
    assert!(app.message.as_ref().is_some_and(|(_, err)| *err));
    press(&mut app, KeyCode::Char('u'));
    type_text(&mut app, PASS);
    press(&mut app, KeyCode::Enter);
    let s = screen(&mut app);
    assert!(s.contains("EUR") && s.contains("vault unlocked"), "{s}");

    // edit the total
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    ctrl(&mut app, 'u');
    type_text(&mut app, "99.5");
    press(&mut app, KeyCode::Enter);
    let snapshots = store(dir.path())
        .query(&Query::new("PortfolioSnapshot"))
        .unwrap();
    let first = snapshots[0].item().unwrap();
    assert_eq!(first.values["total"], 99.5);
    assert_eq!(first.values["positions"][0]["symbol"], "VWCE");

    // a new one, JSON and all
    press(&mut app, KeyCode::Char('n'));
    type_text(&mut app, "2026-09-20");
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "CZK");
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "1000");
    press(&mut app, KeyCode::Tab);
    type_text(
        &mut app,
        r#"[{"symbol": "CEZ", "quantity": 1, "price": 1000}]"#,
    );
    press(&mut app, KeyCode::Enter);
    let snapshots = store(dir.path())
        .query(&Query::new("PortfolioSnapshot").filter("currency", "CZK"))
        .unwrap();
    assert_eq!(snapshots[0].item().unwrap().values["total"], 1000);

    // and locked again, the values go
    press(&mut app, KeyCode::Char('l'));
    assert!(!screen(&mut app).contains("CZK"));
}

#[test]
fn filter_sort_and_delete() {
    let (dir, mut app) = setup();
    press(&mut app, KeyCode::Tab);

    press(&mut app, KeyCode::Char('/'));
    type_text(&mut app, "status=todo");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.rows.len(), 2);
    // free words match the rows' text
    press(&mut app, KeyCode::Char('/'));
    ctrl(&mut app, 'u');
    type_text(&mut app, "CHAR");
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.rows.len(), 1);
    press(&mut app, KeyCode::Char('/'));
    ctrl(&mut app, 'u');
    press(&mut app, KeyCode::Enter);
    assert_eq!(app.rows.len(), 3);

    // s sorts by the first column, S reverses
    press(&mut app, KeyCode::Char('s'));
    press(&mut app, KeyCode::Char('S'));
    let s = screen(&mut app);
    assert!(s.contains("title v"), "{s}");
    assert!(s.find("charlie").unwrap() < s.find("alpha").unwrap());

    // delete the top row, charlie, after a y
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('n'));
    assert!(task(dir.path(), "charlie").is_some(), "n keeps it");
    press(&mut app, KeyCode::Char('d'));
    press(&mut app, KeyCode::Char('y'));
    assert!(task(dir.path(), "charlie").is_none());
    assert_eq!(app.rows.len(), 2);
}

#[test]
fn keys_rebind_from_the_config() {
    let (_dir, mut app) = setup();
    let custom = [("x".to_string(), "quit".to_string())].into();
    app.keymap = Keymap::build(&custom).0;
    press(&mut app, KeyCode::Char('?'));
    assert!(screen(&mut app).contains(" Keys "));
    press(&mut app, KeyCode::Esc);
    press(&mut app, KeyCode::Char('x'));
    assert!(app.quit);
}

#[test]
fn a_server_event_reloads_what_is_shown() {
    let (dir, mut app) = setup();
    press(&mut app, KeyCode::Tab);
    store(dir.path())
        .add_item(
            "Task",
            values(json!({"title": "from elsewhere", "status": "todo"})),
            "x",
        )
        .unwrap();
    assert!(!screen(&mut app).contains("from elsewhere"));
    app.on_event(&strata_server::Event::items("Task"));
    assert!(screen(&mut app).contains("from elsewhere"));
}
