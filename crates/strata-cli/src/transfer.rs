//! `strata export` and `strata import`.
//!
//! Each open type goes out as `<Type>.md`, a table to read in SuperHub,
//! and `<Type>.json`, what import reads back. Vault types go out together
//! in `vault.json.age`, encrypted with the vault passphrase, and never in
//! plaintext; they are exported only while the vault is unlocked.

use std::io::{Read, Write};
use std::path::Path;

use age::secrecy::SecretString;
use anyhow::{Context, bail};
use chrono::{Local, SecondsFormat};
use serde::Serialize;
use serde_json::Value;
use strata_core::{Api, Imported, Partition, TypeExport, VaultStatus};

use crate::config::Config;

pub const VAULT_ARCHIVE: &str = "vault.json.age";

#[derive(Debug, Default, Serialize)]
pub struct Exported {
    pub dir: std::path::PathBuf,
    /// Open types, as markdown and JSON.
    pub open: Vec<String>,
    /// Vault types, in the archive.
    pub vault: Vec<String>,
    /// Vault types left out because the vault is locked.
    pub skipped: Vec<String>,
}

pub fn export(
    store: &dyn Api,
    dir: &Path,
    config: &Config,
    author: &str,
) -> anyhow::Result<Exported> {
    std::fs::create_dir_all(dir).with_context(|| format!("{}", dir.display()))?;
    let unlocked = store.vault_status()? == VaultStatus::Unlocked;
    let mut done = Exported {
        dir: dir.to_path_buf(),
        ..Default::default()
    };
    let mut vault = Vec::new();
    for def in store.list_types()? {
        match def.partition {
            Partition::Open => {
                let t = store.export_type(&def.name)?;
                write_atomic(
                    &dir.join(format!("{}.json", def.name)),
                    &serde_json::to_vec_pretty(&t)?,
                )?;
                let md_path = dir.join(format!("{}.md", def.name));
                let created = existing_created(&md_path);
                write_atomic(&md_path, markdown(&t, created, author).as_bytes())?;
                done.open.push(def.name);
            }
            Partition::Vault if unlocked => {
                vault.push(store.export_type(&def.name)?);
                done.vault.push(def.name);
            }
            Partition::Vault => done.skipped.push(def.name),
        }
    }
    if !vault.is_empty() {
        let passphrase = SecretString::from(config.passphrase(false)?);
        let mut sealed = Vec::new();
        let mut writer =
            age::Encryptor::with_user_passphrase(passphrase).wrap_output(&mut sealed)?;
        writer.write_all(&serde_json::to_vec(&vault)?)?;
        writer.finish()?;
        write_atomic(&dir.join(VAULT_ARCHIVE), &sealed)?;
    }
    Ok(done)
}

/// Every `<Type>.json` in `dir`, and the vault archive if there is one,
/// in one import: all of it or none.
pub fn import(store: &dyn Api, dir: &Path, config: &Config) -> anyhow::Result<Imported> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("{}", dir.display()))?
        .map(|e| e.map(|e| e.path()))
        .collect::<Result<_, _>>()?;
    paths.sort();
    let mut types = Vec::new();
    for path in paths
        .iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
    {
        let text = std::fs::read(path)?;
        let t: TypeExport = serde_json::from_slice(&text)
            .with_context(|| format!("{} is not a strata type export", path.display()))?;
        types.push(t);
    }
    let archive = dir.join(VAULT_ARCHIVE);
    if archive.exists() {
        if store.vault_status()? != VaultStatus::Unlocked {
            return Err(strata_core::Error::VaultLocked)
                .context("the export has vault types, and they go into the vault");
        }
        let sealed = std::fs::read(&archive)?;
        let identity = age::scrypt::Identity::new(SecretString::from(config.passphrase(false)?));
        let mut plain = Vec::new();
        age::Decryptor::new(&sealed[..])?
            .decrypt(std::iter::once(&identity as &dyn age::Identity))
            .with_context(|| format!("{}: not the vault passphrase", archive.display()))?
            .read_to_end(&mut plain)?;
        let vault: Vec<TypeExport> =
            serde_json::from_slice(&plain).context("the vault archive is not a strata export")?;
        types.extend(vault);
    }
    if types.is_empty() {
        bail!("nothing to import in {}", dir.display());
    }
    Ok(store.import(&types)?)
}

/// A SuperHub reference note: frontmatter, then the type as a table.
fn markdown(t: &TypeExport, created: Option<String>, author: &str) -> String {
    let now = Local::now().to_rfc3339_opts(SecondsFormat::Secs, false);
    let name = &t.def.name;
    let mut s = format!(
        "---\ntitle: \"strata: {name}\"\ntype: reference\ntags: [strata]\ncreated: {}\n\
         modified: {now}\nauthor: {author}\nmodified_by: {author}\n---\n\n# {name}\n\n",
        created.unwrap_or_else(|| now.clone())
    );
    if !t.def.description.is_empty() {
        s.push_str(&format!("{}\n\n", t.def.description));
    }
    let columns: Vec<&str> = t.def.properties.iter().map(|p| p.name.as_str()).collect();
    s.push_str("| id | ");
    s.push_str(&columns.join(" | "));
    s.push_str(" | modified | author |\n|");
    s.push_str(&"---|".repeat(columns.len() + 3));
    s.push('\n');
    for item in &t.items {
        let cells: Vec<String> = columns
            .iter()
            .map(|c| item.values.get(*c).map(cell).unwrap_or_default())
            .collect();
        s.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            item.id,
            cells.join(" | "),
            item.modified,
            cell(&Value::String(item.author.clone()))
        ));
    }
    s.push_str(&format!(
        "\n{} items. Written by `strata export`; `{name}.json` beside it is what \
         `strata import` reads, so edit that, not this.\n",
        t.items.len()
    ));
    s
}

fn cell(v: &Value) -> String {
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    text.replace('|', "\\|").replace('\n', "<br>")
}

/// The `created:` of a note written before, so a re-export keeps it.
fn existing_created(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let front = text.strip_prefix("---\n")?.split("\n---").next()?;
    front
        .lines()
        .find_map(|l| l.strip_prefix("created:"))
        .map(|v| v.trim().to_string())
}

/// Write beside, then rename: a reader (SuperHub's indexer, say) never
/// sees half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let tmp = path.with_extension("tmp~");
    std::fs::write(&tmp, bytes).with_context(|| format!("{}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("{}", path.display()))?;
    Ok(())
}
