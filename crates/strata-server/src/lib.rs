//! strata-server: a long-lived strata store behind a Unix socket, for the
//! desktop and the TUI, and the [`Client`] they talk to it with.
//!
//! The server holds the one connection to the files and the vault's key,
//! so clients never fight over either; the CLI uses it when one is
//! running for the same store.

mod client;
pub mod protocol;
mod server;

pub use client::{Client, Events};
pub use protocol::{Event, EventKind, Info};
pub use server::serve;
