//! The store: types, items and relations over `open.db` and `vault.db`.
//!
//! One connection. `open.db` is its `main` schema; unlocking the vault
//! attaches `vault.db` as `vault`, with its key, and locking detaches it.
//! A write that touches both files is one SQLite transaction, so moving a
//! type between partitions cannot leave its items in both or neither.
//!
//! What `open.db` knows about the vault is the catalogue (every type and
//! its properties, so a locked type still has a name and columns) and the
//! `vault_index`: each vault item's id and type. That is what a
//! [`Locked`] placeholder shows, and all it can.

use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, ErrorCode, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::model::{
    Entry, Item, Kind, Locked, Partition, PropertyDef, TypeDef, Values, VaultStatus, check_name,
    check_property_name,
};
use crate::{Error, Result, schema};

const VAULT_FILE: &str = "vault.db";

pub struct Store {
    pub(crate) conn: Connection,
    /// `None` for an in-memory store, whose vault is in memory too and
    /// gone once locked.
    dir: Option<PathBuf>,
    unlocked: bool,
}

/// A typed link between two items. A relation lives with its source, so
/// one from a vault item is hidden while the vault is locked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    pub source: Uuid,
    pub target: Uuid,
    pub kind: String,
}

/// Where an item lives.
struct Location {
    type_name: String,
    partition: Partition,
}

impl Store {
    /// Open the store in `dir`, creating the directory and `open.db` if
    /// they are not there yet. The vault starts locked.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let conn = Connection::open(dir.join("open.db"))?;
        // The rollback journal, not WAL: only with it is a transaction
        // across attached databases atomic as a whole. Set, not assumed,
        // because 0.1 left open.db in WAL and the mode sticks to the file.
        conn.pragma_update(None, "journal_mode", "DELETE")?;
        Self::with_connection(conn, Some(dir.to_path_buf()))
    }

    /// A store that lives and dies with the value: for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::with_connection(Connection::open_in_memory()?, None)
    }

    fn with_connection(conn: Connection, dir: Option<PathBuf>) -> Result<Self> {
        // A wrong key is an answer here, not a fault: keep SQLCipher from
        // printing its decrypt errors to the caller's stderr.
        conn.pragma_update(None, "cipher_log_level", "NONE")?;
        conn.pragma_update(None, "foreign_keys", true)?;
        // a server and a one-shot CLI may share open.db: wait for the
        // other's write rather than fail on it
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // A type moved into the vault must not stay readable in the free
        // pages of open.db: deleted content is overwritten with zeros.
        conn.pragma_update(None, "secure_delete", true)?;
        schema::migrate(&conn, "main")?;
        Ok(Self {
            conn,
            dir,
            unlocked: false,
        })
    }

    /// The directory the store lives in; `None` in memory.
    pub fn dir(&self) -> Option<&Path> {
        self.dir.as_deref()
    }

    // ---- the vault --------------------------------------------------

    pub fn vault_status(&self) -> VaultStatus {
        if self.unlocked {
            VaultStatus::Unlocked
        } else if self.vault_path().is_some_and(|p| p.exists()) {
            VaultStatus::Locked
        } else {
            VaultStatus::Absent
        }
    }

    /// Create the vault, encrypted with `passphrase`, and leave it unlocked.
    pub fn vault_create(&mut self, passphrase: &str) -> Result<()> {
        if self.vault_status() != VaultStatus::Absent {
            return Err(Error::VaultExists);
        }
        self.attach(passphrase)
    }

    /// Unlock the vault. A wrong passphrase is [`Error::WrongPassphrase`]
    /// and leaves the vault locked. Unlocking an unlocked vault is a no-op.
    pub fn vault_unlock(&mut self, passphrase: &str) -> Result<()> {
        match self.vault_status() {
            VaultStatus::Unlocked => Ok(()),
            VaultStatus::Absent => Err(Error::NoVault),
            VaultStatus::Locked => self.attach(passphrase),
        }
    }

    /// Lock the vault: detach it, and with it SQLCipher's key.
    pub fn vault_lock(&mut self) -> Result<()> {
        if self.unlocked {
            // a cached statement that names the vault would keep it open
            self.conn.flush_prepared_statement_cache();
            self.conn.execute_batch("DETACH DATABASE vault")?;
            self.unlocked = false;
        }
        Ok(())
    }

    fn vault_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(VAULT_FILE))
    }

    fn attach(&mut self, passphrase: &str) -> Result<()> {
        let path = self
            .vault_path()
            .map_or(":memory:".to_string(), |p| p.to_string_lossy().into_owned());
        let wrong_key = |e: rusqlite::Error| match e {
            rusqlite::Error::SqliteFailure(f, _) if f.code == ErrorCode::NotADatabase => {
                Error::WrongPassphrase(path.clone().into())
            }
            e => e.into(),
        };
        // SQLCipher reads the first page on ATTACH, so a wrong key usually
        // fails here and nothing is attached
        self.conn
            .execute(
                "ATTACH DATABASE ?1 AS vault KEY ?2",
                params![path, passphrase],
            )
            .map_err(wrong_key)?;
        // and if it did not, the first read tells
        let check = self
            .conn
            .query_row("SELECT count(*) FROM vault.sqlite_master", [], |_| Ok(()));
        if let Err(e) = check {
            self.conn.execute_batch("DETACH DATABASE vault")?;
            return Err(wrong_key(e));
        }
        if let Err(e) = schema::migrate(&self.conn, "vault") {
            self.conn.execute_batch("DETACH DATABASE vault")?;
            return Err(e);
        }
        self.unlocked = true;
        self.sweep_vault_relations()?;
        Ok(())
    }

    /// Relations in the vault whose target was deleted while it was
    /// locked: they could not be removed then, so they go now.
    fn sweep_vault_relations(&self) -> Result<()> {
        self.conn.execute(
            "DELETE FROM vault.relation WHERE
               target NOT IN (SELECT id FROM main.item)
               AND target NOT IN (SELECT id FROM main.vault_index)",
            [],
        )?;
        Ok(())
    }

    /// The schema a partition's data is in, if it can be used now.
    pub(crate) fn schema(&self, partition: Partition) -> Result<&'static str> {
        match (partition, self.vault_status()) {
            (Partition::Open, _) | (Partition::Vault, VaultStatus::Unlocked) => {
                Ok(partition.schema())
            }
            (Partition::Vault, VaultStatus::Locked) => Err(Error::VaultLocked),
            (Partition::Vault, VaultStatus::Absent) => Err(Error::NoVault),
        }
    }

    /// The catalogues a type's definition is written to: always the open
    /// one, and the vault's own for a vault type.
    fn catalogues(&self, partition: Partition) -> Result<&'static [&'static str]> {
        self.schema(partition)?;
        Ok(match partition {
            Partition::Open => &["main"],
            Partition::Vault => &["main", "vault"],
        })
    }

    /// The schemas that can be read now.
    fn readable(&self) -> &'static [&'static str] {
        if self.unlocked {
            &["main", "vault"]
        } else {
            &["main"]
        }
    }

    // ---- types ------------------------------------------------------

    /// Declare a type. A vault type needs the vault unlocked.
    pub fn add_type(&self, def: &TypeDef) -> Result<()> {
        check_name(&def.name)?;
        for (i, p) in def.properties.iter().enumerate() {
            check_property_name(&p.name)?;
            p.check_choices()?;
            if def.properties[..i].iter().any(|q| q.name == p.name) {
                return Err(Error::PropertyExists {
                    type_name: def.name.clone(),
                    property: p.name.clone(),
                });
            }
        }
        let catalogues = self.catalogues(def.partition)?;
        let tx = self.conn.unchecked_transaction()?;
        let exists = tx
            .query_row(
                "SELECT 1 FROM main.type WHERE name = ?1",
                [&def.name],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if exists {
            return Err(Error::TypeExists(def.name.clone()));
        }
        for db in catalogues {
            tx.execute(
                &format!(
                    "INSERT INTO {db}.type (name, partition, description) VALUES (?1, ?2, ?3)"
                ),
                params![def.name, def.partition.as_str(), def.description],
            )?;
            for (position, p) in def.properties.iter().enumerate() {
                insert_property(&tx, db, &def.name, p, position)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn list_types(&self) -> Result<Vec<TypeDef>> {
        let names = self
            .conn
            .prepare("SELECT name FROM main.type ORDER BY name")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        names.iter().map(|n| self.get_type(n)).collect()
    }

    /// A type's definition, readable whether or not the vault is locked.
    pub fn get_type(&self, name: &str) -> Result<TypeDef> {
        let (partition, description): (String, String) = self
            .conn
            .query_row(
                "SELECT partition, description FROM main.type WHERE name = ?1",
                [name],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?
            .ok_or_else(|| Error::UnknownType(name.to_string()))?;
        let mut stmt = self.conn.prepare_cached(
            "SELECT name, kind, required, choices FROM main.property
             WHERE type = ?1 ORDER BY position",
        )?;
        let properties = stmt
            .query_map([name], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, bool>(2)?,
                    r.get::<_, Option<String>>(3)?,
                ))
            })?
            .map(|row| {
                let (name, kind, required, choices) = row?;
                let choices = choices
                    .map(|c| serde_json::from_str(&c))
                    .transpose()
                    .map_err(|e| Error::Corrupt(format!("choices of {name}: {e}")))?;
                Ok(PropertyDef {
                    name,
                    kind: Kind::parse(&kind)?,
                    required,
                    choices,
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
        let def = self.get_type(type_name)?;
        let tx = self.conn.unchecked_transaction()?;
        for db in self.catalogues(def.partition)? {
            tx.execute(
                &format!("UPDATE {db}.type SET description = ?2 WHERE name = ?1"),
                params![type_name, description],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Add a property at the end of the type's list. A required one is
    /// refused while the type has items, since none of them has it.
    pub fn add_property(&self, type_name: &str, property: &PropertyDef) -> Result<()> {
        check_property_name(&property.name)?;
        property.check_choices()?;
        let def = self.get_type(type_name)?;
        let catalogues = self.catalogues(def.partition)?;
        if def.get(&property.name).is_some() {
            return Err(Error::PropertyExists {
                type_name: type_name.to_string(),
                property: property.name.clone(),
            });
        }
        if property.required {
            let data = def.partition.schema();
            let count: i64 = self.conn.query_row(
                &format!("SELECT count(*) FROM {data}.item WHERE type = ?1"),
                [type_name],
                |r| r.get(0),
            )?;
            if count > 0 {
                return Err(Error::RequiredUnmet {
                    property: property.name.clone(),
                    count,
                });
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        for db in catalogues {
            insert_property(&tx, db, type_name, property, def.properties.len())?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Remove a property and every value items hold for it.
    pub fn remove_property(&self, type_name: &str, property: &str) -> Result<()> {
        let def = self.get_type(type_name)?;
        let catalogues = self.catalogues(def.partition)?;
        if def.get(property).is_none() {
            return Err(unknown_property(type_name, property));
        }
        let data = def.partition.schema();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            &format!(
                "DELETE FROM {data}.value WHERE property = ?2
                   AND item IN (SELECT id FROM {data}.item WHERE type = ?1)"
            ),
            params![type_name, property],
        )?;
        for db in catalogues {
            tx.execute(
                &format!("DELETE FROM {db}.property WHERE type = ?1 AND name = ?2"),
                params![type_name, property],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Rename a property, keeping its values.
    pub fn rename_property(&self, type_name: &str, from: &str, to: &str) -> Result<()> {
        check_property_name(to)?;
        let def = self.get_type(type_name)?;
        let catalogues = self.catalogues(def.partition)?;
        if def.get(from).is_none() {
            return Err(unknown_property(type_name, from));
        }
        if def.get(to).is_some() {
            return Err(Error::PropertyExists {
                type_name: type_name.to_string(),
                property: to.to_string(),
            });
        }
        let data = def.partition.schema();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            &format!(
                "UPDATE {data}.value SET property = ?3 WHERE property = ?2
                   AND item IN (SELECT id FROM {data}.item WHERE type = ?1)"
            ),
            params![type_name, from, to],
        )?;
        for db in catalogues {
            tx.execute(
                &format!("UPDATE {db}.property SET name = ?3 WHERE type = ?1 AND name = ?2"),
                params![type_name, from, to],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Make a property required or optional. Required is refused while
    /// any item of the type lacks a value for it.
    pub fn set_required(&self, type_name: &str, property: &str, required: bool) -> Result<()> {
        let def = self.get_type(type_name)?;
        let catalogues = self.catalogues(def.partition)?;
        if def.get(property).is_none() {
            return Err(unknown_property(type_name, property));
        }
        if required {
            let data = def.partition.schema();
            let count: i64 = self.conn.query_row(
                &format!(
                    "SELECT count(*) FROM {data}.item i WHERE i.type = ?1 AND NOT EXISTS
                       (SELECT 1 FROM {data}.value v WHERE v.item = i.id AND v.property = ?2)"
                ),
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
        let tx = self.conn.unchecked_transaction()?;
        for db in catalogues {
            tx.execute(
                &format!("UPDATE {db}.property SET required = ?3 WHERE type = ?1 AND name = ?2"),
                params![type_name, property, required],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Close a text property to `choices`, or open it again with `None`.
    /// Refused while any item holds a value outside them.
    pub fn set_choices(
        &self,
        type_name: &str,
        property: &str,
        choices: Option<Vec<String>>,
    ) -> Result<()> {
        let def = self.get_type(type_name)?;
        let catalogues = self.catalogues(def.partition)?;
        let mut p = def
            .get(property)
            .cloned()
            .ok_or_else(|| unknown_property(type_name, property))?;
        p.choices = choices;
        p.check_choices()?;
        let json = choices_json(&p.choices);
        if let Some(json) = &json {
            let data = def.partition.schema();
            let count: i64 = self.conn.query_row(
                &format!(
                    "SELECT count(*) FROM {data}.value v JOIN {data}.item i ON v.item = i.id
                     WHERE i.type = ?1 AND v.property = ?2
                       AND json_extract(v.value, '$') NOT IN (SELECT value FROM json_each(?3))"
                ),
                params![type_name, property, json],
                |r| r.get(0),
            )?;
            if count > 0 {
                return Err(Error::ChoicesUnmet {
                    property: property.to_string(),
                    count,
                });
            }
        }
        let tx = self.conn.unchecked_transaction()?;
        for db in catalogues {
            tx.execute(
                &format!("UPDATE {db}.property SET choices = ?3 WHERE type = ?1 AND name = ?2"),
                params![type_name, property, json],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Move a type, and every item of it, to the other partition. Items
    /// keep their ids, and relations from them go along. It needs the
    /// vault unlocked either way, and happens in one transaction across
    /// both databases.
    pub fn move_type(&self, type_name: &str, to: Partition) -> Result<()> {
        let def = self.get_type(type_name)?;
        if def.partition == to {
            return Ok(());
        }
        self.schema(Partition::Vault)?;
        let (from, into) = (def.partition.schema(), to.schema());
        let items = format!("(SELECT id FROM {from}.item WHERE type = ?1)");
        let tx = self.conn.unchecked_transaction()?;
        if to == Partition::Vault {
            tx.execute(
                "INSERT INTO vault.type SELECT * FROM main.type WHERE name = ?1",
                [type_name],
            )?;
            tx.execute(
                "INSERT INTO vault.property SELECT * FROM main.property WHERE type = ?1",
                [type_name],
            )?;
        }
        for sql in [
            format!("INSERT INTO {into}.item SELECT * FROM {from}.item WHERE type = ?1"),
            format!("INSERT INTO {into}.value SELECT * FROM {from}.value WHERE item IN {items}"),
            format!(
                "INSERT OR IGNORE INTO {into}.relation
                 SELECT * FROM {from}.relation WHERE source IN {items}"
            ),
            format!("DELETE FROM {from}.relation WHERE source IN {items}"),
        ] {
            tx.execute(&sql, [type_name])?;
        }
        if to == Partition::Vault {
            tx.execute(
                "INSERT INTO main.vault_index SELECT id, type FROM vault.item WHERE type = ?1",
                [type_name],
            )?;
        }
        // values go with their items, by cascade
        tx.execute(
            &format!("DELETE FROM {from}.item WHERE type = ?1"),
            [type_name],
        )?;
        if to == Partition::Open {
            for sql in [
                "DELETE FROM main.vault_index WHERE type = ?1",
                "DELETE FROM vault.property WHERE type = ?1",
                "DELETE FROM vault.type WHERE name = ?1",
            ] {
                tx.execute(sql, [type_name])?;
            }
        }
        for db in ["main", "vault"] {
            tx.execute(
                &format!("UPDATE {db}.type SET partition = ?2 WHERE name = ?1"),
                params![type_name, to.as_str()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ---- items ------------------------------------------------------

    /// Create an item of `type_name`. Null values are the same as leaving
    /// the property out.
    pub fn add_item(&self, type_name: &str, values: Values, author: &str) -> Result<Item> {
        let mut items = self.add_items(type_name, vec![values], author)?;
        Ok(items.remove(0))
    }

    /// Create several items of `type_name`, all or none: one that fails
    /// validation adds nothing.
    pub fn add_items(
        &self,
        type_name: &str,
        bodies: Vec<Values>,
        author: &str,
    ) -> Result<Vec<Item>> {
        check_author(author)?;
        let def = self.get_type(type_name)?;
        let db = self.schema(def.partition)?;
        let bodies = bodies
            .into_iter()
            .map(|values| {
                let values = validate(&def, values)?;
                match def
                    .properties
                    .iter()
                    .find(|p| p.required && !values.contains_key(&p.name))
                {
                    Some(p) => Err(Error::MissingRequired {
                        property: p.name.clone(),
                    }),
                    None => Ok(values),
                }
            })
            .collect::<Result<Vec<_>>>()?;
        let now = now();
        let mut ids = Vec::with_capacity(bodies.len());
        let tx = self.conn.unchecked_transaction()?;
        for values in &bodies {
            let id = Uuid::now_v7();
            tx.execute(
                &format!(
                    "INSERT INTO {db}.item (id, type, created, modified, author, modified_by)
                     VALUES (?1, ?2, ?3, ?3, ?4, ?4)"
                ),
                params![id.to_string(), type_name, now, author],
            )?;
            for (property, value) in values {
                put_value(&tx, db, id, property, value)?;
            }
            if def.partition == Partition::Vault {
                tx.execute(
                    "INSERT INTO main.vault_index (id, type) VALUES (?1, ?2)",
                    params![id.to_string(), type_name],
                )?;
            }
            ids.push(id);
        }
        tx.commit()?;
        ids.into_iter().map(|id| self.read_item(db, id)).collect()
    }

    /// An item with its values. One in the locked vault is
    /// [`Error::VaultLocked`]; [`Store::get`] gives a placeholder instead.
    pub fn get_item(&self, id: Uuid) -> Result<Item> {
        let at = self.locate(id)?;
        self.read_item(self.schema(at.partition)?, id)
    }

    /// An item, or a [`Locked`] placeholder if it is in the locked vault.
    pub fn get(&self, id: Uuid) -> Result<Entry> {
        let at = self.locate(id)?;
        match self.schema(at.partition) {
            Ok(db) => Ok(Entry::Item(self.read_item(db, id)?)),
            Err(Error::VaultLocked) => Ok(Entry::Locked(Locked {
                id,
                type_name: at.type_name,
                locked: true,
            })),
            Err(e) => Err(e),
        }
    }

    /// Merge `patch` into the item's values: a key sets that property, a
    /// null removes it, and properties the patch does not name stay.
    pub fn update_item(&self, id: Uuid, patch: Values, author: &str) -> Result<Item> {
        check_author(author)?;
        let at = self.locate(id)?;
        let db = self.schema(at.partition)?;
        let def = self.get_type(&at.type_name)?;
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
        let tx = self.conn.unchecked_transaction()?;
        for (property, _) in &unset {
            tx.execute(
                &format!("DELETE FROM {db}.value WHERE item = ?1 AND property = ?2"),
                params![id.to_string(), property],
            )?;
        }
        for (property, value) in &set {
            put_value(&tx, db, id, property, value)?;
        }
        tx.execute(
            &format!("UPDATE {db}.item SET modified = ?2, modified_by = ?3 WHERE id = ?1"),
            params![id.to_string(), now(), author],
        )?;
        tx.commit()?;
        self.read_item(db, id)
    }

    /// Delete an item, its values, and every relation it is an end of.
    /// Relations in the locked vault that point at it go at the next
    /// unlock.
    pub fn delete_item(&self, id: Uuid) -> Result<()> {
        let at = self.locate(id)?;
        let db = self.schema(at.partition)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            &format!("DELETE FROM {db}.item WHERE id = ?1"),
            [id.to_string()],
        )?;
        tx.execute(
            "DELETE FROM main.vault_index WHERE id = ?1",
            [id.to_string()],
        )?;
        for db in self.readable() {
            tx.execute(
                &format!("DELETE FROM {db}.relation WHERE source = ?1 OR target = ?1"),
                [id.to_string()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn locate(&self, id: Uuid) -> Result<Location> {
        let found = self
            .conn
            .prepare_cached(
                "SELECT type, 'open' FROM main.item WHERE id = ?1
                 UNION ALL SELECT type, 'vault' FROM main.vault_index WHERE id = ?1",
            )?
            .query_row([id.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })
            .optional()?;
        let (type_name, partition) = found.ok_or(Error::UnknownItem(id))?;
        Ok(Location {
            type_name,
            partition: Partition::parse(&partition),
        })
    }

    pub(crate) fn read_item(&self, db: &str, id: Uuid) -> Result<Item> {
        let mut item = self
            .conn
            .prepare_cached(&format!(
                "SELECT id, type, created, modified, author, modified_by
                 FROM {db}.item WHERE id = ?1"
            ))?
            .query_row([id.to_string()], item_row)
            .optional()?
            .ok_or(Error::UnknownItem(id))?;
        let mut stmt = self.conn.prepare_cached(&format!(
            "SELECT property, value FROM {db}.value WHERE item = ?1"
        ))?;
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

    // ---- relations --------------------------------------------------

    /// Link `source` to `target`. Both must exist; the target may be in
    /// the locked vault, the source may not, since the relation is
    /// written where the source lives.
    pub fn relate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        check_name(kind)?;
        let db = self.schema(self.locate(source)?.partition)?;
        self.locate(target)?;
        self.conn.execute(
            &format!(
                "INSERT OR IGNORE INTO {db}.relation (source, target, kind) VALUES (?1, ?2, ?3)"
            ),
            params![source.to_string(), target.to_string(), kind],
        )?;
        Ok(())
    }

    pub fn unrelate(&self, source: Uuid, target: Uuid, kind: &str) -> Result<()> {
        let db = self.schema(self.locate(source)?.partition)?;
        self.conn.execute(
            &format!("DELETE FROM {db}.relation WHERE source = ?1 AND target = ?2 AND kind = ?3"),
            params![source.to_string(), target.to_string(), kind],
        )?;
        Ok(())
    }

    /// Every relation `id` is an end of, outgoing and incoming, that can
    /// be read now. Resolve either end with [`Store::get`].
    pub fn relations(&self, id: Uuid) -> Result<Vec<Relation>> {
        let mut out = Vec::new();
        for db in self.readable() {
            let mut stmt = self.conn.prepare_cached(&format!(
                "SELECT source, target, kind FROM {db}.relation WHERE source = ?1 OR target = ?1"
            ))?;
            let rows = stmt.query_map([id.to_string()], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?;
            for row in rows {
                let (source, target, kind) = row?;
                out.push(Relation {
                    source: parse_id(&source)?,
                    target: parse_id(&target)?,
                    kind,
                });
            }
        }
        out.sort_by(|a, b| (a.source, &a.kind, a.target).cmp(&(b.source, &b.kind, b.target)));
        Ok(out)
    }
}

fn insert_property(
    conn: &Connection,
    db: &str,
    type_name: &str,
    p: &PropertyDef,
    position: usize,
) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO {db}.property (type, name, kind, required, position, choices)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        ),
        params![
            type_name,
            p.name,
            p.kind.as_str(),
            p.required,
            position as i64,
            choices_json(&p.choices),
        ],
    )?;
    Ok(())
}

fn choices_json(choices: &Option<Vec<String>>) -> Option<String> {
    choices.as_ref().map(|c| serde_json::json!(c).to_string())
}

fn put_value(conn: &Connection, db: &str, id: Uuid, property: &str, value: &Value) -> Result<()> {
    conn.execute(
        &format!(
            "INSERT INTO {db}.value (item, property, value) VALUES (?1, ?2, ?3)
             ON CONFLICT (item, property) DO UPDATE SET value = excluded.value"
        ),
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
        if !p.allows(&value) {
            return Err(Error::NotAChoice {
                property,
                choices: p.choices.clone().unwrap_or_default(),
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
