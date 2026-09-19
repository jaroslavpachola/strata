//! strata-core: the store behind every strata front end.
//!
//! Two SQLite files, `open.db` in plain SQLite and `vault.db` under
//! SQLCipher, carrying one meta-schema and shown to callers as one store.

mod cipher;
mod error;
mod model;
mod query;
mod schema;
mod store;

pub use cipher::open_encrypted;
pub use error::{Error, Result};
pub use model::{Entry, Item, Kind, Locked, Partition, PropertyDef, TypeDef, Values, VaultStatus};
pub use query::{Query, Sort};
pub use store::{Relation, Store};
pub use uuid::Uuid;
