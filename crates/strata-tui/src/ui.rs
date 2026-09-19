//! Drawing: the list of types, the current type's table, the status line
//! and the key bar, and a dialog over them for a form, the passphrase, a
//! delete to confirm, or help.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState,
};
use strata_core::{Entry, Partition, VaultStatus};

use crate::app::{App, Form, Mode, row_cells};

const WIDEST_COLUMN: usize = 40;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let theme = app.theme;
    let area = frame.area();
    frame.render_widget(Block::new().style(theme.base()), area);
    let [main, status, keys] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let list_width = app
        .types
        .iter()
        .map(|t| {
            t.name.len()
                + if t.partition == Partition::Vault {
                    8
                } else {
                    0
                }
        })
        .max()
        .unwrap_or(5)
        .max(5) as u16
        + 4;
    let [types_area, table_area] =
        Layout::horizontal([Constraint::Length(list_width), Constraint::Min(10)]).areas(main);
    draw_types(frame, app, types_area);
    draw_table(frame, app, table_area);
    draw_status(frame, app, status);
    draw_keys(frame, app, keys);

    match &app.mode {
        Mode::Form(form) => draw_form(frame, app, form),
        Mode::Passphrase(text) => {
            let masked = "*".repeat(text.chars().count());
            let lines = vec![
                Line::from(format!("Passphrase: {masked}_")),
                Line::from(""),
                Line::from("Enter to unlock, Esc to leave it locked"),
            ];
            dialog(frame, app, " Unlock the vault ", lines, 50);
        }
        Mode::ConfirmDelete(id) => {
            let lines = vec![
                Line::from(format!("Delete {id}?")),
                Line::from(""),
                Line::from("y to delete, anything else to keep it"),
            ];
            dialog(frame, app, " Delete ", lines, 60);
        }
        Mode::Help => {
            let lines = HELP.iter().map(|l| Line::from(*l)).collect();
            dialog(frame, app, " Keys ", lines, 64);
        }
        Mode::Browse | Mode::Filter(_) => {}
    }
}

fn draw_types(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let items: Vec<ListItem> = app
        .types
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let mut spans = vec![Span::raw(t.name.clone())];
            if t.partition == Partition::Vault {
                let style = if app.vault == VaultStatus::Unlocked {
                    Style::new()
                } else {
                    Style::new().fg(theme.locked_fg)
                };
                spans.push(Span::styled(" (vault)", style));
            }
            let style = if i == app.current {
                theme.selected()
            } else {
                theme.base()
            };
            ListItem::new(Line::from(spans)).style(style)
        })
        .collect();
    let block = Block::new()
        .borders(Borders::ALL)
        .title(" types ")
        .style(theme.base());
    frame.render_widget(List::new(items).block(block), area);
}

fn draw_table(frame: &mut Frame, app: &mut App, area: Rect) {
    let theme = app.theme;
    let columns = app.columns();
    let cells: Vec<Vec<String>> = app
        .rows
        .iter()
        .map(|&i| row_cells(&app.entries[i], &columns))
        .collect();

    let widths: Vec<Constraint> = columns
        .iter()
        .enumerate()
        .map(|(c, name)| {
            let widest = cells
                .iter()
                .map(|r| r[c].chars().count())
                .max()
                .unwrap_or(0);
            Constraint::Length(widest.max(name.len() + 2).min(WIDEST_COLUMN) as u16)
        })
        .collect();
    let header = Row::new(columns.iter().map(|c| {
        let mark = match &app.sort {
            Some(s) if s.by == *c && s.descending => " v",
            Some(s) if s.by == *c => " ^",
            _ => "",
        };
        Cell::from(format!("{c}{mark}"))
    }))
    .style(theme.header());
    let rows: Vec<Row> = app
        .rows
        .iter()
        .zip(cells)
        .map(|(&i, cells)| {
            let row = Row::new(cells);
            match app.entries[i] {
                Entry::Locked(_) => row.style(Style::new().fg(theme.locked_fg)),
                Entry::Item(_) => row,
            }
        })
        .collect();

    let mut title = match app.current_type() {
        Some(t) => format!(" {} · {} ", t.name, app.rows.len()),
        None => " no types: strata init ".to_string(),
    };
    if !app.filter.is_empty() {
        title.push_str(&format!("· filter: {} ", app.filter));
    }
    let block = Block::new()
        .borders(Borders::ALL)
        .title(title)
        .style(theme.base());
    app.page = area.height.saturating_sub(3).max(1) as usize;
    let table = Table::new(rows, widths)
        .header(header)
        .block(block)
        .row_highlight_style(theme.selected());
    let mut state =
        TableState::default().with_selected((!app.rows.is_empty()).then_some(app.selected));
    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let line = match (&app.mode, &app.message) {
        (Mode::Filter(text), _) => Line::from(vec![
            Span::styled("filter: ", Style::new().fg(theme.prompt_fg)),
            Span::raw(format!("{text}_")),
            Span::styled(
                "   property=value filters the store, other words the rows",
                Style::new().fg(theme.fg),
            ),
        ]),
        (_, Some((text, true))) => Line::styled(text.clone(), theme.error()),
        (_, Some((text, false))) => Line::raw(text.clone()),
        (_, None) => Line::raw(format!(
            "vault {}",
            match app.vault {
                VaultStatus::Absent => "absent",
                VaultStatus::Locked => "locked",
                VaultStatus::Unlocked => "unlocked",
            }
        )),
    };
    frame.render_widget(Paragraph::new(line).style(theme.base()), area);
}

fn draw_keys(frame: &mut Frame, app: &App, area: Rect) {
    let theme = app.theme;
    let pairs: &[(&str, &str)] = match app.mode {
        Mode::Form(_) => &[
            ("Enter", "Save"),
            ("Esc", "Cancel"),
            ("Tab", "Next"),
            ("<>", "Choose"),
        ],
        _ => &[
            ("?", "Help"),
            ("Enter", "Open"),
            ("n", "New"),
            ("d", "Delete"),
            ("s", "Sort"),
            ("/", "Filter"),
            ("u", "Unlock"),
            ("Tab", "Type"),
            ("q", "Quit"),
        ],
    };
    let mut spans = Vec::new();
    for (key, label) in pairs {
        spans.push(Span::styled(
            *key,
            Style::new().fg(theme.key_fg).bg(theme.key_bg),
        ));
        spans.push(Span::styled(
            format!("{label} "),
            Style::new().fg(theme.label_fg).bg(theme.label_bg),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)).style(theme.base()), area);
}

fn draw_form(frame: &mut Frame, app: &App, form: &Form) {
    let label_width = form
        .fields
        .iter()
        .map(|f| f.prop.name.len())
        .max()
        .unwrap_or(4);
    let lines = form
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let mut hint = f.prop.kind.as_str().to_string();
            if let Some(c) = &f.prop.choices {
                hint = c.join("|");
            }
            if f.prop.required {
                hint.push('!');
            }
            let cursor = if i == form.focus { "_" } else { "" };
            let text = format!("{:<label_width$}  {}{cursor}", f.prop.name, f.input);
            let line = Line::from(vec![
                Span::raw(text),
                Span::styled(format!("  {hint}"), Style::new().fg(app.theme.prompt_fg)),
            ]);
            if i == form.focus {
                line.style(app.theme.selected())
            } else {
                line
            }
        })
        .collect();
    let title = match form.id {
        Some(_) => format!(" {}: edit ", form.type_name),
        None => format!(" {}: new ", form.type_name),
    };
    dialog(frame, app, &title, lines, 72);
}

fn dialog(frame: &mut Frame, app: &App, title: &str, lines: Vec<Line>, width: u16) {
    let area = frame.area();
    let width = width.min(area.width.saturating_sub(4));
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let rect = Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, rect);
    let block = Block::new()
        .borders(Borders::ALL)
        .title(title.to_string())
        .style(app.theme.dialog());
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

const HELP: &[&str] = &[
    "Up/Down, PgUp/PgDn, Home/End   move",
    "Tab, Shift+Tab, Left/Right     the next or previous type",
    "Enter, F4                      open the item in a form",
    "n, Insert                      a new item",
    "d, F8                          delete the item",
    "s / S                          sort by the next column / reverse",
    "/, Ctrl+F                      filter: status=todo, or any words",
    "u / l                          unlock / lock the vault",
    "Ctrl+R                         reload",
    "q, F10                         quit",
    "",
    "In a form: Tab/arrows to move, type to edit, Left/Right",
    "through choices, Enter saves, Esc cancels.",
    "Keys rebind under [keys] in the config: \"x\" = \"quit\".",
];
