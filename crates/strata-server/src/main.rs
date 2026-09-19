use std::path::PathBuf;

use clap::Parser;
use strata_core::{Store, paths};

/// strata-server: the strata store behind a Unix socket.
#[derive(Parser)]
#[command(name = "strata-server", version)]
struct Args {
    /// The store's directory [default: $XDG_DATA_HOME/strata]
    #[arg(long, env = "STRATA_DIR")]
    dir: Option<PathBuf>,
    /// The socket to listen on [default: $XDG_RUNTIME_DIR/strata.sock]
    #[arg(long, env = "STRATA_SOCKET")]
    socket: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let dir = match args.dir {
        Some(d) => d,
        None => paths::default_dir()?,
    };
    let socket = match args.socket {
        Some(s) => s,
        None => paths::default_socket()?,
    };
    let store = Store::open(&dir)?;
    eprintln!("strata-server: {} on {}", dir.display(), socket.display());
    strata_server::serve(store, &socket, shutdown()).await?;
    Ok(())
}

/// Ctrl-C or SIGTERM: stop taking requests, end the event streams, and
/// remove the socket.
async fn shutdown() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("a SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
