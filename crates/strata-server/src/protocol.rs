//! What goes over the socket.
//!
//! `POST /v1/rpc` takes one [`Request`], tagged by `op`, and answers with
//! the operation's result as JSON, or an [`ErrorBody`] with a non-2xx
//! status. `GET /v1/events` streams [`Event`]s as server-sent events,
//! `?type=T` for one type's. Both ends are this crate, so the enum is the
//! contract; curl works too:
//!
//! ```text
//! curl --unix-socket $XDG_RUNTIME_DIR/strata.sock \
//!      -d '{"op": "query", "query": {"type": "Task"}}' http://strata/v1/rpc
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use strata_core::{Partition, PropertyDef, Query, TypeDef, TypeExport, Uuid, Values};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Info,
    VaultStatus,
    VaultCreate {
        passphrase: String,
    },
    VaultUnlock {
        passphrase: String,
    },
    VaultLock,
    AddType {
        def: TypeDef,
    },
    ListTypes,
    GetType {
        name: String,
    },
    SetDescription {
        type_name: String,
        description: String,
    },
    AddProperty {
        type_name: String,
        property: PropertyDef,
    },
    RemoveProperty {
        type_name: String,
        property: String,
    },
    RenameProperty {
        type_name: String,
        from: String,
        to: String,
    },
    SetRequired {
        type_name: String,
        property: String,
        required: bool,
    },
    SetChoices {
        type_name: String,
        property: String,
        choices: Option<Vec<String>>,
    },
    MoveType {
        type_name: String,
        to: Partition,
    },
    AddItems {
        type_name: String,
        bodies: Vec<Values>,
        author: String,
    },
    GetItem {
        id: Uuid,
    },
    Get {
        id: Uuid,
    },
    UpdateItem {
        id: Uuid,
        patch: Values,
        author: String,
    },
    DeleteItem {
        id: Uuid,
    },
    Relate {
        source: Uuid,
        target: Uuid,
        kind: String,
    },
    Unrelate {
        source: Uuid,
        target: Uuid,
        kind: String,
    },
    Relations {
        id: Uuid,
    },
    Query {
        query: Query,
    },
    Seed,
    SeedNeedsVault,
    ExportType {
        type_name: String,
    },
    Import {
        types: Vec<TypeExport>,
    },
}

/// The answer to [`Request::Info`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Info {
    pub dir: PathBuf,
    pub version: String,
}

/// A failed request: [`strata_core::Error::code`] and its message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorBody {
    pub code: String,
    pub error: String,
}

/// Something changed. A pane showing `type_name` redraws on `Items` or
/// `Type` for its type, and on every `Vault` event, which flips vault
/// items between values and placeholders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub kind: EventKind,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    /// Items of the type were added, changed, deleted or related.
    Items,
    /// The type's definition changed, or it moved partition.
    Type,
    /// The vault was created, unlocked or locked.
    Vault,
}

impl Event {
    pub fn items(type_name: impl Into<String>) -> Self {
        Self {
            kind: EventKind::Items,
            type_name: Some(type_name.into()),
        }
    }

    pub fn type_changed(type_name: impl Into<String>) -> Self {
        Self {
            kind: EventKind::Type,
            type_name: Some(type_name.into()),
        }
    }

    pub fn vault() -> Self {
        Self {
            kind: EventKind::Vault,
            type_name: None,
        }
    }

    /// Whether a subscriber to `type_name` (or to everything) wants it.
    pub fn concerns(&self, type_name: Option<&str>) -> bool {
        match (type_name, &self.type_name) {
            (None, _) | (_, None) => true,
            (Some(want), Some(got)) => want == got,
        }
    }
}
