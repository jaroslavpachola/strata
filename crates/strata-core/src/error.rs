use std::path::PathBuf;

use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The passphrase did not open the vault. SQLCipher cannot tell a
    /// wrong key from a file that is not a database, so neither can we.
    #[error("wrong passphrase, or {0} is not a strata vault")]
    WrongPassphrase(PathBuf),
    #[error("the vault is locked")]
    VaultLocked,
    #[error("there is no vault yet: create one first")]
    NoVault,
    #[error("a vault already exists")]
    VaultExists,
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
    #[error("{0}: choices need a text property and at least one choice")]
    BadChoices(String),
    #[error("{property} is not one of {choices:?}, got {value}")]
    NotAChoice {
        property: String,
        choices: Vec<String>,
        value: serde_json::Value,
    },
    #[error("{property} cannot take those choices: {count} item(s) hold another value")]
    ChoicesUnmet { property: String, count: i64 },
    #[error("every write needs an author")]
    NoAuthor,
    #[error("type {0:?} exists with another definition and has items")]
    TypeConflict(String),
    #[error("item {0} already exists")]
    ItemExists(Uuid),
    #[error("the database holds something strata did not write: {0}")]
    Corrupt(String),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// An error the server reported that has no variant of its own here.
    #[error("{message}")]
    Remote { code: String, message: String },
    /// The server could not be reached or answered nonsense.
    #[error("strata-server: {0}")]
    Server(String),
}

impl Error {
    /// A stable name for the error, for the wire and for callers that
    /// would rather match on a string than on the enum.
    pub fn code(&self) -> &str {
        match self {
            Error::WrongPassphrase(_) => "wrong_passphrase",
            Error::VaultLocked => "vault_locked",
            Error::NoVault => "no_vault",
            Error::VaultExists => "vault_exists",
            Error::UnknownType(_) => "unknown_type",
            Error::TypeExists(_) => "type_exists",
            Error::UnknownProperty { .. } => "unknown_property",
            Error::PropertyExists { .. } => "property_exists",
            Error::UnknownItem(_) => "unknown_item",
            Error::InvalidName(_) => "invalid_name",
            Error::ReservedName(_) => "reserved_name",
            Error::InvalidValue { .. } => "invalid_value",
            Error::MissingRequired { .. } => "missing_required",
            Error::RequiredUnmet { .. } => "required_unmet",
            Error::BadChoices(_) => "bad_choices",
            Error::NotAChoice { .. } => "not_a_choice",
            Error::ChoicesUnmet { .. } => "choices_unmet",
            Error::NoAuthor => "no_author",
            Error::TypeConflict(_) => "type_conflict",
            Error::ItemExists(_) => "item_exists",
            Error::Corrupt(_) => "corrupt",
            Error::Sqlite(_) => "sqlite",
            Error::Io(_) => "io",
            Error::Remote { code, .. } => code,
            Error::Server(_) => "server",
        }
    }

    /// An error as the server reported it. The vault's states come back as
    /// themselves, so a caller can tell locked from absent without the
    /// code; the rest keep their code and message.
    pub fn from_remote(code: &str, message: &str) -> Self {
        match code {
            "vault_locked" => Error::VaultLocked,
            "no_vault" => Error::NoVault,
            "vault_exists" => Error::VaultExists,
            "no_author" => Error::NoAuthor,
            _ => Error::Remote {
                code: code.to_string(),
                message: message.to_string(),
            },
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
