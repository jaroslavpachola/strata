//! Export and import: a type with its items and their relations, as data
//! that survives the store, so a schema change can go out, be edited,
//! and come back in.

use std::collections::HashSet;

use rusqlite::params;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::model::{Item, Partition, TypeDef, check_name, check_property_name};
use crate::query::Query;
use crate::store::{Relation, insert_property, put_value, validate};
use crate::{Error, Result, Store};

/// One type, whole: its definition, every item, and the relations whose
/// source is one of them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeExport {
    #[serde(rename = "type")]
    pub def: TypeDef,
    #[serde(default)]
    pub items: Vec<Item>,
    #[serde(default)]
    pub relations: Vec<Relation>,
}

/// What an import did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Imported {
    /// Types declared, or redeclared over an empty one.
    pub types: Vec<String>,
    pub items: usize,
    pub relations: usize,
}

impl Store {
    /// A type and everything in it. A vault type needs the vault unlocked.
    pub fn export_type(&self, type_name: &str) -> Result<TypeExport> {
        let def = self.get_type(type_name)?;
        self.schema(def.partition)?;
        let items: Vec<Item> = self
            .query(&Query::new(type_name))?
            .into_iter()
            .filter_map(|e| e.into_item())
            .collect();
        let mut relations = Vec::new();
        for item in &items {
            relations.extend(
                self.relations(item.id)?
                    .into_iter()
                    .filter(|r| r.source == item.id),
            );
        }
        Ok(TypeExport {
            def,
            items,
            relations,
        })
    }

    /// Bring exported types back, all or nothing, ids, times and authors
    /// as they were. A type the store already has is kept if its
    /// definition is the same, replaced if it has no items, and a
    /// [`Error::TypeConflict`] otherwise. An item whose id exists is
    /// [`Error::ItemExists`]; every value is checked as on any write.
    pub fn import(&self, types: &[TypeExport]) -> Result<Imported> {
        let mut done = Imported::default();
        let mut seen = HashSet::new();
        let tx = self.conn.unchecked_transaction()?;

        for t in types {
            let def = &t.def;
            match self.get_type(&def.name) {
                Ok(existing) if existing == *def => {}
                Ok(existing) => {
                    let count: i64 = tx.query_row(
                        "SELECT (SELECT count(*) FROM main.item WHERE type = ?1)
                              + (SELECT count(*) FROM main.vault_index WHERE type = ?1)",
                        [&def.name],
                        |r| r.get(0),
                    )?;
                    if count > 0 {
                        return Err(Error::TypeConflict(def.name.clone()));
                    }
                    for db in self.catalogues(existing.partition)? {
                        tx.execute(
                            &format!("DELETE FROM {db}.property WHERE type = ?1"),
                            [&def.name],
                        )?;
                        tx.execute(
                            &format!("DELETE FROM {db}.type WHERE name = ?1"),
                            [&def.name],
                        )?;
                    }
                    self.declare(&tx, def)?;
                    done.types.push(def.name.clone());
                }
                Err(Error::UnknownType(_)) => {
                    self.declare(&tx, def)?;
                    done.types.push(def.name.clone());
                }
                Err(e) => return Err(e),
            }

            let db = self.schema(def.partition)?;
            for item in &t.items {
                if item.type_name != def.name {
                    return Err(Error::Corrupt(format!(
                        "item {} is a {}, in the export of {}",
                        item.id, item.type_name, def.name
                    )));
                }
                if !seen.insert(item.id) || self.get(item.id).is_ok() {
                    return Err(Error::ItemExists(item.id));
                }
                let values = validate(def, item.values.clone())?;
                if let Some(p) = def
                    .properties
                    .iter()
                    .find(|p| p.required && !values.contains_key(&p.name))
                {
                    return Err(Error::MissingRequired {
                        property: p.name.clone(),
                    });
                }
                tx.execute(
                    &format!(
                        "INSERT INTO {db}.item (id, type, created, modified, author, modified_by)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
                    ),
                    params![
                        item.id.to_string(),
                        def.name,
                        item.created,
                        item.modified,
                        item.author,
                        item.modified_by
                    ],
                )?;
                for (property, value) in &values {
                    put_value(&tx, db, item.id, property, value)?;
                }
                if def.partition == Partition::Vault {
                    tx.execute(
                        "INSERT INTO main.vault_index (id, type) VALUES (?1, ?2)",
                        params![item.id.to_string(), def.name],
                    )?;
                }
                done.items += 1;
            }
        }

        // relations last: a target may be in a type imported after its source
        for t in types {
            let db = t.def.partition.schema();
            let sources: HashSet<Uuid> = t.items.iter().map(|i| i.id).collect();
            for r in &t.relations {
                if !sources.contains(&r.source) {
                    return Err(Error::Corrupt(format!(
                        "relation from {}, which is not a {}",
                        r.source, t.def.name
                    )));
                }
                check_name(&r.kind)?;
                self.get(r.target)?;
                tx.execute(
                    &format!(
                        "INSERT OR IGNORE INTO {db}.relation (source, target, kind)
                         VALUES (?1, ?2, ?3)"
                    ),
                    params![r.source.to_string(), r.target.to_string(), r.kind],
                )?;
                done.relations += 1;
            }
        }
        tx.commit()?;
        Ok(done)
    }

    /// Write a type's definition into its catalogues, inside a caller's
    /// transaction.
    fn declare(&self, tx: &rusqlite::Connection, def: &TypeDef) -> Result<()> {
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
        for db in self.catalogues(def.partition)? {
            tx.execute(
                &format!(
                    "INSERT INTO {db}.type (name, partition, description) VALUES (?1, ?2, ?3)"
                ),
                params![def.name, def.partition.as_str(), def.description],
            )?;
            for (position, p) in def.properties.iter().enumerate() {
                insert_property(tx, db, &def.name, p, position)?;
            }
        }
        Ok(())
    }
}
