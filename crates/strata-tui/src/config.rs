//! strata-tui's part of the strata config file: `theme` and `[keys]`.
//! The rest of the file is the CLI's, and ignored here.

use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub theme: Option<String>,
    /// `"key" = "action"`, over the defaults.
    #[serde(default)]
    pub keys: BTreeMap<String, String>,
}

impl Config {
    /// A missing file is the defaults; a broken one is the defaults and a
    /// warning, so a typo never keeps the TUI from starting.
    pub fn load(path: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(c) => (c, vec![]),
                Err(e) => (Self::default(), vec![format!("{}: {e}", path.display())]),
            },
            Err(_) => (Self::default(), vec![]),
        }
    }
}
