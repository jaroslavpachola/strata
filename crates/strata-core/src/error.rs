use std::path::PathBuf;

use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The passphrase did not open the vault. SQLCipher cannot tell a
    /// wrong key from a file that is not a database, so neither can we.
    #[error("wrong passphrase, or {0} is not a strata vault")]
    WrongPassphrase(PathBuf),
    #[error("the vault partition is not available yet")]
    VaultUnavailable,
    #[error("no type named {0:?}")]
    UnknownType(String),
    #[error("type {0:?} already exists")]
    TypeExists(String),
    #[error("type {type_name:?} has no property {property:?}")]
    UnknownProperty { type_name: String, property: String },
    #[error("type {type_name:?} already has a property {property:?}")]
    PropertyExists { type_name: String, property: String },
    #[error("no item {0}")]
    UnknownItem(Uuid),
    #[error("{0:?} is not a valid name: a letter or _, then letters, digits or _")]
    InvalidName(String),
    #[error("{0:?} is reserved for the item's own fields")]
    ReservedName(String),
    #[error("{property}: expected {reason}, got {value}")]
    InvalidValue {
        property: String,
        reason: &'static str,
        value: serde_json::Value,
    },
    #[error("{property} is required")]
    MissingRequired { property: String },
    #[error("{property} cannot become required: {count} item(s) have no value for it")]
    RequiredUnmet { property: String, count: i64 },
    #[error("every write needs an author")]
    NoAuthor,
    #[error("the database holds something strata did not write: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
