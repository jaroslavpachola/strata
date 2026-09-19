//! Answers: JSON with `--json`, short text for a person otherwise.

use serde::Serialize;
use serde_json::{Value, json};
use strata_core::{Entry, Item, Relation, TypeDef};

pub struct Out {
    pub json: bool,
}

impl Out {
    /// `v` as JSON, or `human(v)` as text.
    pub fn value<T: Serialize>(
        &self,
        v: &T,
        human: impl FnOnce(&Value) -> String,
    ) -> anyhow::Result<()> {
        let v = serde_json::to_value(v)?;
        if self.json {
            println!("{v}");
        } else {
            println!("{}", human(&v));
        }
        Ok(())
    }

    pub fn types(&self, types: &[TypeDef]) -> anyhow::Result<()> {
        self.value(&types, |_| {
            types
                .iter()
                .map(|t| {
                    let mut s = format!("{} ({})", t.name, t.partition.as_str());
                    if !t.description.is_empty() {
                        s.push_str(&format!("  {}", t.description));
                    }
                    for p in &t.properties {
                        let req = if p.required { "  required" } else { "" };
                        s.push_str(&format!("\n  {:<16} {}{req}", p.name, p.kind.as_str()));
                    }
                    s
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        })
    }

    pub fn item(&self, item: &Item) -> anyhow::Result<()> {
        self.value(item, |_| item_text(item))
    }

    pub fn items(&self, items: &[Item]) -> anyhow::Result<()> {
        self.value(&items, |_| {
            items.iter().map(item_text).collect::<Vec<_>>().join("\n\n")
        })
    }

    /// One line per entry: what a query answers.
    pub fn entries(&self, entries: &[Entry]) -> anyhow::Result<()> {
        self.value(&entries, |_| {
            entries
                .iter()
                .map(entry_line)
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    pub fn relations(&self, relations: &[Relation]) -> anyhow::Result<()> {
        self.value(&relations, |_| {
            relations
                .iter()
                .map(|r| format!("{} -{}-> {}", r.source, r.kind, r.target))
                .collect::<Vec<_>>()
                .join("\n")
        })
    }

    pub fn error(&self, e: &anyhow::Error, locked: bool) {
        let message = format!("{e:#}");
        if self.json {
            eprintln!("{}", json!({"error": message, "locked": locked}));
        } else if locked {
            eprintln!("strata: {message} (pass --unlock)");
        } else {
            eprintln!("strata: {message}");
        }
    }
}

fn item_text(item: &Item) -> String {
    let mut s = format!(
        "{}  {}\n  created  {} by {}",
        item.id, item.type_name, item.created, item.author
    );
    if item.modified != item.created {
        s.push_str(&format!(
            "\n  modified {} by {}",
            item.modified, item.modified_by
        ));
    }
    for (k, v) in &item.values {
        s.push_str(&format!("\n  {k}: {}", show(v)));
    }
    s
}

fn entry_line(entry: &Entry) -> String {
    match entry {
        Entry::Locked(l) => format!("{}  {}  [locked]", l.id, l.type_name),
        Entry::Item(item) => {
            let values: Vec<_> = item
                .values
                .iter()
                .map(|(k, v)| format!("{k}={}", show(v)))
                .collect();
            format!("{}  {}", item.id, values.join("  "))
        }
    }
}

/// A string as itself, anything else as JSON.
fn show(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
