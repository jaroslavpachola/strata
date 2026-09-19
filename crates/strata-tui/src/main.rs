use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;
use clap::Parser;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use strata_core::{Api, Store, paths};
use strata_server::Client;
use strata_tui::app::App;
use strata_tui::config::Config;
use strata_tui::keymap::Keymap;
use strata_tui::theme::Theme;
use strata_tui::ui;

/// strata-tui: browse and edit the strata store.
#[derive(Parser)]
#[command(name = "strata-tui", version)]
struct Args {
    /// The store's directory [default: $XDG_DATA_HOME/strata]
    #[arg(long, env = "STRATA_DIR")]
    dir: Option<PathBuf>,
    /// The config file [default: $XDG_CONFIG_HOME/strata/config.toml]
    #[arg(long, env = "STRATA_CONFIG")]
    config: Option<PathBuf>,
    /// Who is writing [default: $USER]
    #[arg(long, env = "STRATA_AUTHOR")]
    author: Option<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let dir = match args.dir {
        Some(d) => d,
        None => paths::default_dir()?,
    };
    let config_path = match args.config {
        Some(p) => p,
        None => paths::default_config()?,
    };
    let (config, mut warnings) = Config::load(&config_path);
    let (keymap, key_warnings) = Keymap::build(&config.keys);
    warnings.extend(key_warnings);
    let theme = match config.theme.as_deref() {
        None => Theme::mc(),
        Some(name) => Theme::named(name).unwrap_or_else(|| {
            warnings.push(format!("theme {name:?}: mc, dark or bw"));
            Theme::mc()
        }),
    };
    let author = args
        .author
        .or_else(|| std::env::var("USER").ok())
        .context("who is writing? pass --author or set $STRATA_AUTHOR")?;

    // the server if one holds this store, and its events; else the files
    let (store, socket): (Box<dyn Api>, _) = match strata_server::server_for(&dir)? {
        Some(client) => {
            let socket = client.socket().to_path_buf();
            (Box::new(client), Some(socket))
        }
        None => {
            anyhow::ensure!(
                dir.join("open.db").exists(),
                "no store at {}: run `strata init`",
                dir.display()
            );
            (Box::new(Store::open(&dir)?), None)
        }
    };
    let (tx, rx) = mpsc::channel();
    if let Some(socket) = socket {
        std::thread::spawn(move || {
            let Ok(client) = Client::connect(&socket) else {
                return;
            };
            let Ok(events) = client.events(None) else {
                return;
            };
            for event in events.flatten() {
                if tx.send(event).is_err() {
                    break;
                }
            }
        });
    }

    let mut app = App::new(store, author, keymap, theme);
    app.env_passphrase = std::env::var("STRATA_VAULT_PASSPHRASE").ok();
    if !warnings.is_empty() {
        app.warn(warnings.join("; "));
    }

    let mut terminal = ratatui::init();
    let result = (|| -> anyhow::Result<()> {
        while !app.quit {
            terminal.draw(|f| ui::draw(f, &mut app))?;
            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                app.handle_key(key);
            }
            while let Ok(event) = rx.try_recv() {
                app.on_event(&event);
            }
        }
        Ok(())
    })();
    ratatui::restore();
    result
}
