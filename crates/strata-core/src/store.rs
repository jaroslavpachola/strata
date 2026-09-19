//! The store: types, items and relations over `open.db`.

use std::path::Path;

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::model::{
    Item, Kind, Partition, PropertyDef, TypeDef, Values, check_name, check_property_name,
};
use crate::{Error, Result, schema};

pub struct Store {
    pub(crate) open: Connection,
}

/// A typed link between two items. Either end may be in either
/// partition, so neither is checked against the other database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: String,
}

impl Store {
    /// Open the store in `dir`, creating the directory and `open.db` if
    /// they are not there yet.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("open.db"))?;
        // WAL: a reader in one process does not block a writer in another
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::with_connection(conn)
    }

    /// A store that lives and dies with the value: for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?)
    }

    fn with_connection(mut conn: Connection) -> Result<Self> {
        schema::migrate(&mut conn)?;
        Ok(Self { open: conn })
    }

    // ---- types ------------------------------------------------------

    pub fn add_type(&self, def: &TypeDef) -> Result<()> {
        check_name(&def.name)?;
        if def.partition == Partition::Vault {
            return Err(Error::VaultUnavailable);
        }
        for (i, p) in def.properties.iter().enumerate() {
            check_property_name(&p.name)?;
            if def.properties[..i].iter().any(|q| q.name == p.name) {
                return Err(Error::PropertyExists {
                    type_name: def.name.clone(),
                    property: p.name.clone(),
                });
            }
        }
        let tx = self.open.unchecked_transaction()?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM type WHERE name = ?1",
                [&def.name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Err(Error::TypeExists(def.name.clone()));
        }
        tx.execute(
            "INSERT INTO type (name, partition, description) VALUES (?1, ?2, ?3)",
            params![def.name, def.partition.as_str(), def.description],
        )?;
        for (position, p) in def.properties.iter().enumerate() {
            insert_property(&tx, &def.name, p, position)?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_types(&self) -> Result<Vec<TypeDef>> {
        let names = self
            .open
            .prepare("SELECT name FROM type ORDER BY name")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        names.iter().map(|n| self.get_type(n)).collect()
    }

    pub fn get_type(&self, name: &str) -> Result<TypeDef> {
        let (partition, description): (String, String) = self
            .open
            .query_row(
                "SELECT partition, description FROM type WHERE name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::UnknownType(name.to_string()))?;
        let mut stmt = self.open.prepare_cached(
            "SELECT name, kind, required FROM property WHERE type = ?1 ORDER BY position",
        )?;
        let properties = stmt
            .query_map([name], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                ))
            })?
            .map(|row| {
                let (name, kind, required) = row?;
                Ok(PropertyDef {
                    name,
                    kind: Kind::parse(&kind)?,
                    required,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TypeDef {
            name: name.to_string(),
            partition: Partition::parse(&partition),
            description,
            properties,
        })
    }

    pub fn set_description(&self, type_name: &str, description: &str) -> Result<()> {
        let n = self.open.execute(
            "UPDATE type SET description = ?2 WHERE name = ?1",
            params![type_name, description],
        )?;
        if n == 0 {
            return Err(Error::UnknownType(type_name.to_string()));
        }
        Ok(())
    }

    /// Add a property at the end of the type's list. A required one is
    /// refused while the type has items, since none of them has it.
    pub fn add_property(&self, type_name: &str, property: &PropertyDef) -> Result<()> {
        check_property_name(&property.name)?;
        let def = self.get_type(type_name)?;
        if def.get(&property.name).is_some() {
            return Err(Error::PropertyExists {
                type_name: type_name.to_string(),
                property: property.name.clone(),
            });
        }
        if property.required {
            let count = self.count_items(type_name)?;
            if count > 0 {
                return Err(Error::RequiredUnmet {
                    property: property.name.clone(),
                    count,
                });
            }
        }
        insert_property(&self.open, type_name, property, def.properties.len())
    }

    /// Remove a property and every value items hold for it.
    pub fn remove_property(&self, type_name: &str, property: &str) -> Result<()> {
        self.property(type_name, property)?;
        let tx = self.open.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM value WHERE property = ?2
               AND item IN (SELECT id FROM item WHERE type = ?1)",
            params![type_name, property],
        )?;
        tx.execute(
            "DELETE FROM property WHERE type = ?1 AND name = ?2",
            params![type_name, property],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rename a property, keeping its values.
    pub fn rename_property(&self, type_name: &str, from: &str, to: &str) -> Result<()> {
        check_property_name(to)?;
        let def = self.get_type(type_name)?;
        if def.get(from).is_none() {
            return Err(unknown_property(type_name, from));
        }
        if def.get(to).is_some() {
            return Err(Error::PropertyExists {
                type_name: type_name.to_string(),
                property: to.to_string(),
            });
        }
        let tx = self.open.unchecked_transaction()?;
        tx.execute(
            "UPDATE value SET property = ?3 WHERE property = ?2
               AND item IN (SELECT id FROM item WHERE type = ?1)",
            params![type_name, from, to],
        )?;
        tx.execute(
            "UPDATE property SET name = ?3 WHERE type = ?1 AND name = ?2",
            params![type_name, from, to],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Make a property required or optional. Required is refused while
    /// any item of the type lacks a value for it.
    pub fn set_required(&self, type_name: &str, property: &str, required: bool) -> Result<()> {
        self.property(type_name, property)?;
        if required {
            let count: i64 = self.open.query_row(
                "SELECT count(*) FROM item i WHERE i.type = ?1 AND NOT EXISTS
                   (SELECT 1 FROM value v WHERE v.item = i.id AND v.property = ?2)",
                params![type_name, property],
                |r| r.get(0),
            )?;
            if count > 0 {
                return Err(Error::RequiredUnmet {
                    property: property.to_string(),
                    count,
                });
            }
        }
        self.open.execute(
            "UPDATE property SET required = ?3 WHERE type = ?1 AND name = ?2",
            params![type_name, property, required],
        )?;
        Ok(())
    }

    fn property(&self, type_name: &str, property: &str) -> Result<PropertyDef> {
        self.get_type(type_name)?
            .get(property)
            .cloned()
            .ok_or_else(|| unknown_property(type_name, property))
    }

    fn count_items(&self, type_name: &str) -> Result<i64> {
        Ok(self.open.query_row(
            "SELECT count(*) FROM item WHERE type = ?1",
            [type_name],
            |r| r.get(0),
        )?)
    }

    // ---- items ------------------------------------------------------

    /// Create an item of `type_name`. Null values are the same as leaving
    /// the property out.
    pub fn add_item(&self, type_name: &str, values: Values, author: &str) -> Result<Item> {
        check_author(author)?;
        let def = self.get_type(type_name)?;
        let values = validate(&def, values)?;
        if let Some(p) = def
            .properties
            .iter()
            .find(|p| p.required && !values.contains_key(&p.name))
        {
            return Err(Error::MissingRequired {
                property: p.name.clone(),
            });
        }
        let id = Uuid::now_v7();
        let now = now();
        let tx = self.open.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO item (id, type, created, modified, author, modified_by)
             VALUES (?1, ?2, ?3, ?3, ?4, ?4)",
            params![id.to_string(), type_name, now, author],
        )?;
        for (property, value) in &values {
            put_value(&tx, id, property, value)?;
        }
        tx.commit()?;
        self.get_item(id)
    }

    pub fn get_item(&self, id: Uuid) -> Result<Item> {
        let mut item = self
            .open
            .prepare_cached(
                "SELECT id, type, created, modified, author, modified_by FROM item WHERE id = ?1",
            )?
            .query_row([id.to_string()], item_row)
            .optional()?
            .ok_or(Error::UnknownItem(id))?;
        let mut stmt = self
            .open
            .prepare_cached("SELECT property, value FROM value WHERE item = ?1")?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (property, json) = row?;
            let value = serde_json::from_str(&json)
                .map_err(|e| Error::Corrupt(format!("value of {property} on {id}: {e}")))?;
            item.values.insert(property, value);
        }
        Ok(item)
    }

    /// Merge `patch` into the item's values: a key sets that property, a
    /// null removes it, and properties the patch does not name stay.
    pub fn update_item(&self, id: Uuid, patch: Values, author: &str) -> Result<Item> {
        check_author(author)?;
        let item = self.get_item(id)?;
        let def = self.get_type(&item.type_name)?;
        let (unset, set): (Vec<_>, Vec<_>) = patch.into_iter().partition(|(_, v)| v.is_null());
        for (property, _) in &unset {
            match def.get(property) {
                None => return Err(unknown_property(&def.name, property)),
                Some(p) if p.required => {
                    return Err(Error::MissingRequired {
                        property: property.clone(),
                    });
                }
                Some(_) => {}
            }
        }
        let set = validate(&def, set.into_iter().collect())?;
        let tx = self.open.unchecked_transaction()?;
        for (property, _) in &unset {
            tx.execute(
                "DELETE FROM value WHERE item = ?1 AND property = ?2",
                params![id.to_string(), property],
            )?;
        }
        for (property, value) in &set {
            put_value(&tx, id, property, value)?;
        }
        tx.execute(
            "UPDATE item SET modified = ?2, modified_by = ?3 WHERE id = ?1",
            params![id.to_string(), now(), author],
        )?;
        tx.commit()?;
        self.get_item(id)
    }

    /// Delete an item, its values, and every relation it is an end of.
    pub fn delete_item(&self, id: Uuid) -> Result<()> {
        let tx = self.open.unchecked_transaction()?;
        let n = tx.execute("DELETE FROM item WHERE id = ?1", [id.to_string()])?;
        if n == 0 {
            return Err(Error::UnknownItem(id));
        }
        tx.execute(
            "DELETE FROM relation WHERE source = ?1 OR target = ?1",
            [id.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }

    // ---- relations --------------------------------------------------

    /// Link `source` to `target`. The source must be in the store; the
    /// target is taken on trust, because it may sit in a locked vault.
    pub fn relate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        check_name(kind)?;
        self.get_item(source)?;
        self.open.execute(
            "INSERT OR IGNORE INTO relation (source, target, kind) VALUES (?1, ?2, ?3)",
            params![source.to_string(), target.to_string(), kind],
        )?;
        Ok(())
    }

    pub fn unrelate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        self.open.execute(
            "DELETE FROM relation WHERE source = ?1 AND target = ?2 AND kind = ?3",
            params![source.to_string(), target.to_string(), kind],
        )?;
        Ok(())
    }

    /// Every relation `id` is an end of, outgoing and incoming.
    pub fn relations(&self, id: Uuid) -> Result<Vec<Relation>> {
        let mut stmt = self.open.prepare_cached(
            "SELECT source, target, kind FROM relation WHERE source = ?1 OR target = ?1
             ORDER BY source, kind, target",
        )?;
        let rows = stmt.query_map([id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?;
        rows.map(|row| {
            let (source, target, kind) = row?;
            Ok(Relation {
                source: parse_id(&source)?,
                target: parse_id(&target)?,
                kind,
            })
        })
        .collect()
    }
}

fn insert_property(
    conn: &Connection,
    type_name: &str,
    p: &PropertyDef,
    position: usize,
) -> Result<()> {
    conn.execute(
        "INSERT INTO property (type, name, kind, required, position) VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            type_name,
            p.name,
            p.kind.as_str(),
            p.required,
            position as i64
        ],
    )?;
    Ok(())
}

fn put_value(conn: &Connection, id: Uuid, property: &str, value: &Value) -> Result<()> {
    conn.execute(
        "INSERT INTO value (item, property, value) VALUES (?1, ?2, ?3)
         ON CONFLICT (item, property) DO UPDATE SET value = excluded.value",
        params![id.to_string(), property, value.to_string()],
    )?;
    Ok(())
}

/// Check every value against its property's kind, dropping nulls.
fn validate(def: &TypeDef, values: Values) -> Result<Values> {
    let mut out = Values::new();
    for (property, value) in values {
        let p = def
            .get(&property)
            .ok_or_else(|| unknown_property(&def.name, &property))?;
        if value.is_null() {
            continue;
        }
        if let Some(reason) = p.kind.reject(&value) {
            return Err(Error::InvalidValue {
                property,
                reason,
                value,
            });
        }
        out.insert(property, value);
    }
    Ok(out)
}

/// An item row with no values yet. The id was written by
/// [`Store::add_item`], so one that does not parse is the database's
/// fault and surfaces as a conversion error.
fn item_row(r: &Row) -> rusqlite::Result<Item> {
    let id: String = r.get(0)?;
    Ok(Item {
        id: Uuid::parse_str(&id).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, e.into())
        })?,
        type_name: r.get(1)?,
        created: r.get(2)?,
        modified: r.get(3)?,
        author: r.get(4)?,
        modified_by: r.get(5)?,
        values: Values::new(),
    })
}

pub(crate) fn parse_id(s: &str) -> Result<Uuid> {
    Uuid::parse_str(s).map_err(|e| Error::Corrupt(format!("item id {s:?}: {e}")))
}

fn unknown_property(type_name: &str, property: &str) -> Error {
    Error::UnknownProperty {
        type_name: type_name.to_string(),
        property: property.to_string(),
    }
}

fn check_author(author: &str) -> Result<()> {
    if author.trim().is_empty() {
        Err(Error::NoAuthor)
    } else {
        Ok(())
    }
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}
