//! Queries: one type, equality on properties, a sort, a page.
//!
//! No expression language yet. When JSON filtering gets slow, typed
//! value columns and a small grammar come in together.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use crate::model::{Item, Values};
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
    pub fn query(&self, q: &Query) -> Result<Vec<Item>> {
        let def = self.get_type(&q.type_name)?;
        let known = |property: &str| {
            def.get(property)
                .map(|_| ())
                .ok_or_else(|| Error::UnknownProperty {
                    type_name: def.name.clone(),
                    property: property.to_string(),
                })
        };

        let mut sql = String::from("SELECT i.id FROM item i WHERE i.type = ?");
        let mut args: Vec<rusqlite::types::Value> = vec![q.type_name.clone().into()];

        for (property, value) in &q.filter {
            known(property)?;
            args.push(property.clone().into());
            if value.is_null() {
                sql.push_str(
                    " AND NOT EXISTS (SELECT 1 FROM value v WHERE v.item = i.id AND v.property = ?)",
                );
            } else {
                // json_extract on both sides: 2 and 2.0 compare equal, and
                // strings compare as strings rather than as quoted JSON
                sql.push_str(
                    " AND EXISTS (SELECT 1 FROM value v WHERE v.item = i.id AND v.property = ?
                        AND json_extract(v.value, '$') = json_extract(?, '$'))",
                );
                args.push(value.to_string().into());
            }
        }

        let (key, descending) = match &q.sort {
            None => ("i.created".to_string(), false),
            Some(s) => {
                let key = match s.by.as_str() {
                    "created" => "i.created".to_string(),
                    "modified" => "i.modified".to_string(),
                    property => {
                        known(property)?;
                        args.push(property.to_string().into());
                        "(SELECT json_extract(v.value, '$') FROM value v
                           WHERE v.item = i.id AND v.property = ?)"
                            .to_string()
                    }
                };
                (key, s.descending)
            }
        };
        let dir = if descending { "DESC" } else { "ASC" };
        // the id breaks ties: UUIDv7, so that is creation order again
        sql.push_str(&format!(" ORDER BY {key} {dir} NULLS LAST, i.id {dir}"));

        if q.limit.is_some() || q.offset.is_some() {
            sql.push_str(" LIMIT ? OFFSET ?");
            args.push(q.limit.map_or(-1, |n| n as i64).into());
            args.push((q.offset.unwrap_or(0) as i64).into());
        }

        let ids = self
            .open
            .prepare(&sql)?
            .query_map(rusqlite::params_from_iter(args), |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.iter()
            .map(|id| parse_id(id).and_then(|id: Uuid| self.get_item(id)))
            .collect()
    }
}
