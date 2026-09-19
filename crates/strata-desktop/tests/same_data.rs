//! The M8 gate: the egui browser shows and edits the same data the TUI
//! does. Each front end holds its own handle on one store directory; what
//! one writes, the other shows.

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde_json::json;
use strata_core::{Store, Values};
use strata_egui::StrataBrowser;
use strata_tui::app::App;
use strata_tui::keymap::Keymap;
use strata_tui::theme::Theme;

const PASS: &str = "same data";

struct Pane {
    browser: StrataBrowser,
    store: Store,
}

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_text(app: &mut App, text: &str) {
    for c in text.chars() {
        press(app, KeyCode::Char(c));
    }
}

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(140, 30)).unwrap();
    terminal.draw(|f| strata_tui::ui::draw(f, app)).unwrap();
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

fn click(h: &mut Harness<'_, Pane>, label: &str) {
    h.get_by_label(label).click();
    h.run();
}

#[test]
fn what_one_front_end_writes_the_other_shows() {
    let dir = tempfile::tempdir().unwrap();
    let mut seed = Store::open(dir.path()).unwrap();
    seed.vault_create(PASS).unwrap();
    seed.seed().unwrap();
    seed.add_items(
        "Task",
        vec![
            values(json!({"title": "alpha", "status": "todo"})),
            values(json!({"title": "bravo", "status": "todo"})),
        ],
        "seed",
    )
    .unwrap();
    seed.add_item(
        "PortfolioSnapshot",
        values(json!({"taken_at": "2026-09-19", "currency": "EUR", "total": 10, "positions": []})),
        "seed",
    )
    .unwrap();
    seed.vault_lock().unwrap();
    drop(seed);

    let mut desktop = Harness::builder()
        .with_size(egui::vec2(1200.0, 800.0))
        .build_ui_state(
            |ui, pane: &mut Pane| pane.browser.show(ui, &mut pane.store),
            Pane {
                browser: StrataBrowser::new("desktop"),
                store: Store::open(dir.path()).unwrap(),
            },
        );
    let mut tui = App::new(
        Box::new(Store::open(dir.path()).unwrap()),
        "tui".into(),
        Keymap::default(),
        Theme::mc(),
    );

    // the desktop moves bravo to doing; the TUI shows it
    desktop.run();
    click(&mut desktop, "Task");
    click(&mut desktop, "bravo");
    click(&mut desktop, "Open");
    click(&mut desktop, "status *");
    click(&mut desktop, "doing");
    click(&mut desktop, "Save");
    press(&mut tui, KeyCode::Tab);
    let s = screen(&mut tui);
    let bravo = s.lines().find(|l| l.contains("bravo")).unwrap();
    assert!(bravo.contains("doing"), "{s}");

    // the TUI renames alpha; the desktop shows it
    press(&mut tui, KeyCode::Enter);
    tui.handle_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
    type_text(&mut tui, "alpha, renamed in the tui");
    press(&mut tui, KeyCode::Enter);
    desktop.state_mut().browser.invalidate();
    desktop.run();
    assert!(
        desktop
            .query_by_label("alpha, renamed in the tui")
            .is_some()
    );

    // and the vault: the TUI unlocks its handle and changes the total,
    // the desktop unlocks its own and sees it
    press(&mut tui, KeyCode::BackTab);
    press(&mut tui, KeyCode::Char('u'));
    type_text(&mut tui, PASS);
    press(&mut tui, KeyCode::Enter);
    press(&mut tui, KeyCode::Enter);
    press(&mut tui, KeyCode::Down);
    press(&mut tui, KeyCode::Down);
    type_text(&mut tui, "0");
    press(&mut tui, KeyCode::Enter);
    click(&mut desktop, "PortfolioSnapshot (locked)");
    desktop.get_by_label("vault locked, passphrase:").focus();
    desktop.run();
    desktop
        .get_by_label("vault locked, passphrase:")
        .type_text(PASS);
    desktop.run();
    click(&mut desktop, "Unlock");
    assert!(desktop.query_by_label("100").is_some(), "10 and a typed 0");
}
