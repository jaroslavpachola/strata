//! Queries: one type, equality on properties, a sort, a page.
//!
//! No expression language yet. When JSON filtering gets slow, typed
//! value columns and a small grammar come in together.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::model::{Entry, Locked, Values};
use crate::store::{Store, parse_id};
use crate::{Error, Result};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Query {
    #[serde(rename = "type")]
    pub type_name: String,
    /// Property equals value, all of them. A null matches items that
    /// have no value for the property.
    #[serde(default, skip_serializing_if = "Values::is_empty")]
    pub filter: Values,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sort: Option<Sort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sort {
    /// `created`, `modified`, or a property name. Items with no value
    /// for the property come last either way.
    pub by: String,
    #[serde(default)]
    pub descending: bool,
}

impl Query {
    pub fn new(type_name: impl Into<String>) -> Self {
        Self {
            type_name: type_name.into(),
            ..Self::default()
        }
    }

    pub fn filter(mut self, property: impl Into<String>, value: impl Into<Value>) -> Self {
        self.filter.insert(property.into(), value.into());
        self
    }

    pub fn sort_by(mut self, by: impl Into<String>) -> Self {
        self.sort = Some(Sort {
            by: by.into(),
            descending: false,
        });
        self
    }

    /// Reverse the sort; a query with none sorts by `created`.
    pub fn descending(mut self) -> Self {
        let by = self.sort.take().map_or_else(|| "created".into(), |s| s.by);
        self.sort = Some(Sort {
            by,
            descending: true,
        });
        self
    }

    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }
}

impl Store {
    /// Items of one type. A vault type queried while the vault is locked
    /// yields a [`Locked`] placeholder per item, in creation order; a
    /// filter or a sort that would need its values is
    /// [`Error::VaultLocked`] instead, since answering it would say
    /// something about them.
    pub fn query(&self, q: &Query) -> Result<Vec<Entry>> {
        let def = self.get_type(&q.type_name)?;
        let known = |property: &str| {
            def.get(property)
                .map(|_| ())
                .ok_or_else(|| Error::UnknownProperty {
                    type_name: def.name.clone(),
                    property: property.to_string(),
                })
        };
        for property in q.filter.keys() {
            known(property)?;
        }
        let sort = q.sort.clone().unwrap_or(Sort {
            by: "created".into(),
            descending: false,
        });
        if !matches!(sort.by.as_str(), "created" | "modified") {
            known(&sort.by)?;
        }
        let dir = if sort.descending { "DESC" } else { "ASC" };

        let db = match self.schema(def.partition) {
            Err(Error::VaultLocked) => {
                if !q.filter.is_empty() || sort.by != "created" {
                    return Err(Error::VaultLocked);
                }
                return self.locked_placeholders(&def.name, dir, q);
            }
            other => other?,
        };

        let mut sql = format!("SELECT i.id FROM {db}.item i WHERE i.type = ?");
        let mut args: Vec<rusqlite::types::Value> = vec![q.type_name.clone().into()];

        for (property, value) in &q.filter {
            args.push(property.clone().into());
            if value.is_null() {
                sql.push_str(&format!(
                    " AND NOT EXISTS (SELECT 1 FROM {db}.value v
                        WHERE v.item = i.id AND v.property = ?)"
                ));
            } else {
                // json_extract on both sides: 2 and 2.0 compare equal, and
                // strings compare as strings rather than as quoted JSON
                sql.push_str(&format!(
                    " AND EXISTS (SELECT 1 FROM {db}.value v WHERE v.item = i.id AND v.property = ?
                        AND json_extract(v.value, '$') = json_extract(?, '$'))"
                ));
                args.push(value.to_string().into());
            }
        }

        let key = match sort.by.as_str() {
            "created" => "i.created".to_string(),
            "modified" => "i.modified".to_string(),
            property => {
                args.push(property.to_string().into());
                format!(
                    "(SELECT json_extract(v.value, '$') FROM {db}.value v
                       WHERE v.item = i.id AND v.property = ?)"
                )
            }
        };
        // the id breaks ties: UUIDv7, so that is creation order again
        sql.push_str(&format!(" ORDER BY {key} {dir} NULLS LAST, i.id {dir}"));
        page(&mut sql, &mut args, q);

        let ids = self.ids(&sql, args)?;
        ids.into_iter()
            .map(|id| self.read_item(db, id).map(Entry::Item))
            .collect()
    }

    fn locked_placeholders(&self, type_name: &str, dir: &str, q: &Query) -> Result<Vec<Entry>> {
        let mut sql = format!("SELECT id FROM main.vault_index WHERE type = ? ORDER BY id {dir}");
        let mut args: Vec<rusqlite::types::Value> = vec![type_name.to_string().into()];
        page(&mut sql, &mut args, q);
        Ok(self
            .ids(&sql, args)?
            .into_iter()
            .map(|id| {
                Entry::Locked(Locked {
                    id,
                    type_name: type_name.to_string(),
                    locked: true,
                })
            })
            .collect())
    }

    fn ids(&self, sql: &str, args: Vec<rusqlite::types::Value>) -> Result<Vec<Uuid>> {
        let ids = self
            .conn
            .prepare(sql)?
            .query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.iter().map(|id| parse_id(id)).collect()
    }
}

fn page(sql: &mut String, args: &mut Vec<rusqlite::types::Value>, q: &Query) {
    if q.limit.is_some() || q.offset.is_some() {
        sql.push_str(" LIMIT ? OFFSET ?");
        args.push(q.limit.map_or(-1, |n| n as i64).into());
        args.push((q.offset.unwrap_or(0) as i64).into());
    }
}
