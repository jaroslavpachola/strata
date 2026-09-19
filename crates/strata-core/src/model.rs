//! What the store holds: types, their properties, items and values.

use chrono::{DateTime, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::{Error, Result};

/// An item's values by property name.
pub type Values = serde_json::Map<String, Value>;

/// Which database a type's items live in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Partition {
    #[default]
    Open,
    Vault,
}

impl Partition {
    pub fn as_str(self) -> &'static str {
        match self {
            Partition::Open => "open",
            Partition::Vault => "vault",
        }
    }

    pub(crate) fn parse(s: &str) -> Self {
        match s {
            "vault" => Partition::Vault,
            _ => Partition::Open,
        }
    }

    /// The schema the partition's tables sit in on the store's connection.
    pub(crate) fn schema(self) -> &'static str {
        match self {
            Partition::Open => "main",
            Partition::Vault => "vault",
        }
    }
}

/// Whether the vault can be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VaultStatus {
    /// No `vault.db` yet.
    Absent,
    Locked,
    Unlocked,
}

/// What a property's value must be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Text,
    Number,
    /// `YYYY-MM-DD`, or an RFC 3339 timestamp.
    Date,
    Bool,
    /// Anything JSON can say.
    Json,
    /// Another item's id.
    Ref,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Number => "number",
            Kind::Date => "date",
            Kind::Bool => "bool",
            Kind::Json => "json",
            Kind::Ref => "ref",
        }
    }

    pub(crate) fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "text" => Kind::Text,
            "number" => Kind::Number,
            "date" => Kind::Date,
            "bool" => Kind::Bool,
            "json" => Kind::Json,
            "ref" => Kind::Ref,
            _ => return Err(Error::Corrupt(format!("unknown property kind {s:?}"))),
        })
    }

    /// What was expected, if `value` is not of this kind. Null is never
    /// checked here: it means "no value" and is handled by the caller.
    pub fn reject(self, value: &Value) -> Option<&'static str> {
        let (ok, expected) = match self {
            Kind::Text => (value.is_string(), "a string"),
            Kind::Number => (value.is_number(), "a number"),
            Kind::Bool => (value.is_boolean(), "true or false"),
            Kind::Json => (true, "JSON"),
            Kind::Date => (
                value.as_str().is_some_and(|s| {
                    NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
                        || DateTime::parse_from_rfc3339(s).is_ok()
                }),
                "YYYY-MM-DD or an RFC 3339 timestamp",
            ),
            Kind::Ref => (
                value.as_str().is_some_and(|s| Uuid::parse_str(s).is_ok()),
                "an item id (a UUID)",
            ),
        };
        (!ok).then_some(expected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyDef {
    pub name: String,
    pub kind: Kind,
    #[serde(default)]
    pub required: bool,
    /// For a text property: the only values it may take.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<Vec<String>>,
}

impl PropertyDef {
    pub fn new(name: impl Into<String>, kind: Kind) -> Self {
        Self {
            name: name.into(),
            kind,
            required: false,
            choices: None,
        }
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn choices<S: Into<String>>(mut self, choices: impl IntoIterator<Item = S>) -> Self {
        self.choices = Some(choices.into_iter().map(Into::into).collect());
        self
    }

    /// Choices only on text, and at least one of them.
    pub(crate) fn check_choices(&self) -> Result<()> {
        match &self.choices {
            None => Ok(()),
            Some(c) if self.kind == Kind::Text && !c.is_empty() => Ok(()),
            Some(_) => Err(Error::BadChoices(self.name.clone())),
        }
    }

    /// Whether `value` (already of the right kind) is one of the choices.
    pub(crate) fn allows(&self, value: &Value) -> bool {
        match (&self.choices, value.as_str()) {
            (Some(c), Some(s)) => c.iter().any(|x| x == s),
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeDef {
    pub name: String,
    #[serde(default)]
    pub partition: Partition,
    #[serde(default)]
    pub description: String,
    /// In declaration order, which is the order a table shows them in.
    #[serde(default)]
    pub properties: Vec<PropertyDef>,
}

impl TypeDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            partition: Partition::Open,
            description: String::new(),
            properties: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = description.into();
        self
    }

    pub fn partition(mut self, partition: Partition) -> Self {
        self.partition = partition;
        self
    }

    pub fn property(mut self, property: PropertyDef) -> Self {
        self.properties.push(property);
        self
    }

    pub fn get(&self, property: &str) -> Option<&PropertyDef> {
        self.properties.iter().find(|p| p.name == property)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub type_name: String,
    /// RFC 3339, UTC, milliseconds: sorts as text.
    pub created: String,
    pub modified: String,
    /// Who created the item: `jarda`, `claude`, a script's name.
    pub author: String,
    /// Who wrote it last.
    pub modified_by: String,
    pub values: Values,
}

/// A vault item seen while the vault is locked: that it exists and what
/// type it is, and nothing of its values, times or authors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Locked {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub type_name: String,
    /// Always true: marks the placeholder for a reader of the JSON.
    pub locked: bool,
}

/// What a query or a relation yields: an item, or a placeholder for one
/// in the locked vault.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Entry {
    Item(Item),
    Locked(Locked),
}

impl Entry {
    pub fn id(&self) -> Uuid {
        match self {
            Entry::Item(item) => item.id,
            Entry::Locked(locked) => locked.id,
        }
    }

    pub fn type_name(&self) -> &str {
        match self {
            Entry::Item(item) => &item.type_name,
            Entry::Locked(locked) => &locked.type_name,
        }
    }

    pub fn item(&self) -> Option<&Item> {
        match self {
            Entry::Item(item) => Some(item),
            Entry::Locked(_) => None,
        }
    }

    pub fn into_item(self) -> Option<Item> {
        match self {
            Entry::Item(item) => Some(item),
            Entry::Locked(_) => None,
        }
    }
}

/// Names the item's own fields take, kept off properties so a sort key
/// or a column heading is never ambiguous.
const RESERVED: &[&str] = &["id", "type", "created", "modified", "author", "modified_by"];

/// Type and property names: an identifier, so they read the same in
/// JSON, a shell and a table heading.
pub(crate) fn check_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else {
        Err(Error::InvalidName(name.to_string()))
    }
}

pub(crate) fn check_property_name(name: &str) -> Result<()> {
    check_name(name)?;
    if RESERVED.contains(&name) {
        return Err(Error::ReservedName(name.to_string()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn kinds_accept_their_own_values_only() {
        assert!(Kind::Text.reject(&json!("x")).is_none());
        assert!(Kind::Text.reject(&json!(1)).is_some());
        assert!(Kind::Number.reject(&json!(1.5)).is_none());
        assert!(Kind::Number.reject(&json!("1")).is_some());
        assert!(Kind::Bool.reject(&json!(false)).is_none());
        assert!(Kind::Date.reject(&json!("2026-09-19")).is_none());
        assert!(
            Kind::Date
                .reject(&json!("2026-09-19T09:00:00+02:00"))
                .is_none()
        );
        assert!(Kind::Date.reject(&json!("19.9.2026")).is_some());
        assert!(Kind::Json.reject(&json!({"a": [1]})).is_none());
        assert!(
            Kind::Ref
                .reject(&json!(Uuid::now_v7().to_string()))
                .is_none()
        );
        assert!(Kind::Ref.reject(&json!("nope")).is_some());
    }

    #[test]
    fn names_are_identifiers() {
        assert!(check_name("Task").is_ok());
        assert!(check_name("due_at2").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("2fa").is_err());
        assert!(check_name("with space").is_err());
        assert!(check_property_name("created").is_err());
    }
}
