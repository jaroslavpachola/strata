//! `strata links`: how the store and SuperHub point at each other.
//!
//! An item points at a note through its `note` property, a vault path. A
//! note points at an item by writing `strata://item/<uuid>`. The first
//! direction is in the store; the second is in the notes, read from a
//! SuperHub vault on disk when there is one, else asked of the hub over
//! its API (`SUPERHUB_URL`, `SUPERHUB_API_KEY`, as its MCP server reads
//! them).

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};
use strata_core::{Api, Entry, Kind, Query, Uuid, VaultStatus};

pub const SCHEME: &str = "strata://item/";

#[derive(Debug, Default, Serialize)]
pub struct Links {
    /// Items whose `note` names a note.
    pub items: Vec<ItemNote>,
    /// Notes that link to an item.
    pub notes: Vec<NoteLink>,
    /// Where the notes were read: a directory, the hub's URL, or nothing.
    pub notes_from: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ItemNote {
    pub id: Uuid,
    #[serde(rename = "type")]
    pub type_name: String,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct NoteLink {
    pub note: String,
    pub id: Uuid,
    /// The item's type; `None` when no item has that id any more.
    #[serde(rename = "type")]
    pub type_name: Option<String>,
}

/// Where the notes are.
pub enum Notes {
    Dir(PathBuf),
    Hub { url: String, key: Option<String> },
    Nowhere,
}

pub fn links(store: &dyn Api, notes: &Notes) -> anyhow::Result<Links> {
    let mut out = Links::default();
    let unlocked = store.vault_status()? == VaultStatus::Unlocked;
    for def in store.list_types()? {
        let has_note = def.get("note").is_some_and(|p| p.kind == Kind::Text);
        // a vault item's note is one of its values: shown only unlocked
        if !has_note || (def.partition == strata_core::Partition::Vault && !unlocked) {
            continue;
        }
        for entry in store.query(&Query::new(&def.name))? {
            if let Entry::Item(item) = entry
                && let Some(note) = item.values.get("note").and_then(|v| v.as_str())
            {
                out.items.push(ItemNote {
                    id: item.id,
                    type_name: def.name.clone(),
                    note: note.to_string(),
                });
            }
        }
    }

    let found = match notes {
        Notes::Dir(dir) => {
            out.notes_from = Some(dir.display().to_string());
            from_dir(dir)?
        }
        Notes::Hub { url, key } => {
            out.notes_from = Some(url.clone());
            from_hub(url, key.as_deref())?
        }
        Notes::Nowhere => Vec::new(),
    };
    for (note, id) in found {
        let type_name = store.get(id).ok().map(|e| e.type_name().to_string());
        out.notes.push(NoteLink {
            note,
            id,
            type_name,
        });
    }
    Ok(out)
}

/// Every `strata://item/<uuid>` in `text`.
pub fn item_links(text: &str) -> Vec<Uuid> {
    let mut ids = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(SCHEME) {
        rest = &rest[at + SCHEME.len()..];
        if let Some(id) = rest.get(..36).and_then(|s| Uuid::parse_str(s).ok())
            && !ids.contains(&id)
        {
            ids.push(id);
        }
    }
    ids
}

/// Every markdown file under `dir`, as (vault path, id) pairs.
fn from_dir(dir: &Path) -> anyhow::Result<Vec<(String, Uuid)>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).with_context(|| format!("{}", d.display()))? {
            let path = entry?.path();
            let hidden = path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with('.'));
            if hidden {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let rel = path.strip_prefix(dir).unwrap_or(&path);
                for id in item_links(&text) {
                    out.push((rel.to_string_lossy().into_owned(), id));
                }
            }
        }
    }
    out.sort();
    Ok(out)
}

#[derive(Deserialize)]
struct Found {
    path: String,
}

#[derive(Deserialize)]
struct Detail {
    content: String,
}

/// The hub's regex search for the scheme, then each note that has it.
fn from_hub(url: &str, key: Option<&str>) -> anyhow::Result<Vec<(String, Uuid)>> {
    let url = url.trim_end_matches('/');
    let agent = ureq::Agent::new_with_defaults();
    let get = |u: String| -> anyhow::Result<String> {
        let mut req = agent.get(&u);
        if let Some(key) = key {
            req = req.header("Authorization", &format!("Bearer {key}"));
        }
        let mut resp = req.call().with_context(|| format!("SuperHub: {u}"))?;
        Ok(resp.body_mut().read_to_string()?)
    };
    let search = get(format!(
        "{url}/api/search?mode=regex&limit=100&q={}",
        encode("strata://item/[0-9a-fA-F-]{36}")
    ))?;
    let found: Vec<Found> = serde_json::from_str(&search).context("SuperHub's search answer")?;
    let mut out = Vec::new();
    for f in found {
        let body = get(format!("{url}/api/notes/{}", encode_path(&f.path)))?;
        let detail: Detail = serde_json::from_str(&body).context("SuperHub's note")?;
        for id in item_links(&detail.content) {
            out.push((f.path.clone(), id));
        }
    }
    out.sort();
    Ok(out)
}

/// Percent-encode for a query value.
fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

/// Percent-encode a vault path, keeping its slashes.
fn encode_path(s: &str) -> String {
    s.split('/').map(encode).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_are_found_once_each() {
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let text = format!(
            "see [x]({SCHEME}{a}) and {SCHEME}{b}, again {SCHEME}{a}; not {SCHEME}nonsense"
        );
        assert_eq!(item_links(&text), [a, b]);
    }

    #[test]
    fn paths_keep_their_slashes() {
        assert_eq!(encode_path("Daily/2026 09.md"), "Daily/2026%2009.md");
    }
}
