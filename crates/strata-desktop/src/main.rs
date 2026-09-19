//! strata-desktop: the strata browser in a window of its own, for use
//! before Plocha, and a window that opens in a second to try the widgets
//! in. Through strata-server when one holds the store, redrawing on its
//! events; the files otherwise.

use std::path::PathBuf;
use std::sync::mpsc;

use anyhow::Context;
use clap::Parser;
use strata_core::{Api, Store, paths};
use strata_egui::StrataBrowser;
use strata_server::{Client, Event};

/// strata-desktop: browse and edit the strata store in a window.
#[derive(Parser)]
#[command(name = "strata-desktop", version)]
struct Args {
    /// The store's directory [default: $XDG_DATA_HOME/strata]
    #[arg(long, env = "STRATA_DIR")]
    dir: Option<PathBuf>,
    /// Who is writing [default: $USER]
    #[arg(long, env = "STRATA_AUTHOR")]
    author: Option<String>,
}

struct Desktop {
    store: Box<dyn Api>,
    browser: StrataBrowser,
    events: mpsc::Receiver<Event>,
}

impl eframe::App for Desktop {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if self.events.try_iter().count() > 0 {
            self.browser.invalidate();
        }
        egui::CentralPanel::default().show(ui, |ui| {
            self.browser.show(ui, self.store.as_mut());
        });
    }
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let dir = match args.dir {
        Some(d) => d,
        None => paths::default_dir()?,
    };
    let author = args
        .author
        .or_else(|| std::env::var("USER").ok())
        .context("who is writing? pass --author or set $STRATA_AUTHOR")?;
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

    let (tx, events) = mpsc::channel();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("strata")
            .with_app_id("strata-desktop")
            .with_inner_size([1100.0, 700.0]),
        ..Default::default()
    };
    eframe::run_native(
        "strata",
        options,
        Box::new(move |cc| {
            // the server's events wake the window, which then asks again
            if let Some(socket) = socket {
                let ctx = cc.egui_ctx.clone();
                std::thread::spawn(move || {
                    let Ok(client) = Client::connect(&socket) else {
                        return;
                    };
                    let Ok(stream) = client.events(None) else {
                        return;
                    };
                    for event in stream.flatten() {
                        if tx.send(event).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                });
            }
            Ok(Box::new(Desktop {
                store,
                browser: StrataBrowser::new(author),
                events,
            }))
        }),
    )
    .map_err(|e| anyhow::anyhow!("{e}"))
}
