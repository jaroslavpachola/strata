//! Where the store is, and where the vault passphrase comes from.

use std::cell::OnceCell;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, bail};
use serde::Deserialize;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// `barbero` to ask Barbero for the passphrase before prompting.
    #[serde(default)]
    pub cascade: Cascade,
    #[serde(default = "default_barbero_command")]
    pub barbero_command: String,
    #[serde(default = "default_barbero_entry")]
    pub barbero_entry: String,
    /// Where `strata export` writes [default: the SuperHub vault's
    /// References/strata]
    pub export_dir: Option<PathBuf>,
    /// The SuperHub vault [default: $SUPERHUB_VAULT_PATH]
    pub superhub_vault: Option<PathBuf>,
    /// A SuperHub hub, for `strata links` when the vault is not on this
    /// disk [default: $SUPERHUB_URL]; its key comes from $SUPERHUB_API_KEY
    pub superhub_url: Option<String>,
    /// strata-tui's, read there: the colour theme and its key bindings.
    /// Accepted here so the one file serves both.
    #[serde(default, rename = "theme")]
    _theme: Option<String>,
    #[serde(default, rename = "keys")]
    _keys: std::collections::BTreeMap<String, String>,
    /// The passphrase once found, so one command asks once.
    #[serde(skip)]
    known: OnceCell<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Cascade {
    /// The environment, then a prompt.
    #[default]
    Prompt,
    /// The environment, then Barbero, which prompts for its own master
    /// password.
    Barbero,
}

fn default_barbero_command() -> String {
    "barbero-cli".into()
}

fn default_barbero_entry() -> String {
    "strata/vault".into()
}

const PASSPHRASE_ENV: &str = "STRATA_VAULT_PASSPHRASE";

impl Config {
    /// `$STRATA_CONFIG`, else `$XDG_CONFIG_HOME/strata/config.toml`. A
    /// missing file is the defaults; a broken one is an error.
    pub fn load() -> anyhow::Result<Self> {
        let path = match std::env::var_os("STRATA_CONFIG") {
            Some(p) => PathBuf::from(p),
            None => strata_core::paths::default_config()?,
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str(&text).with_context(|| format!("{}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("{}", path.display())),
        }
    }

    /// The vault passphrase: `$STRATA_VAULT_PASSPHRASE` if set, else
    /// Barbero if the config says so, else a prompt on the terminal. `confirm`
    /// asks twice at the prompt, for a vault about to be created.
    pub fn passphrase(&self, confirm: bool) -> anyhow::Result<String> {
        if let Some(p) = self.known.get() {
            return Ok(p.clone());
        }
        let p = if let Ok(p) = std::env::var(PASSPHRASE_ENV) {
            p
        } else if self.cascade == Cascade::Barbero {
            self.ask_barbero()?
        } else {
            prompt(confirm)?
        };
        Ok(self.known.get_or_init(|| p).clone())
    }

    /// Where `strata links` reads notes: `--vault`, else a SuperHub vault
    /// on disk, else the hub over its API, else nowhere.
    pub fn notes(&self, dir: Option<PathBuf>) -> crate::links::Notes {
        use crate::links::Notes;
        let dir = dir.or_else(|| self.superhub_vault.clone()).or_else(|| {
            std::env::var_os("SUPERHUB_VAULT_PATH")
                .map(PathBuf::from)
                .filter(|p| p.is_dir())
        });
        if let Some(dir) = dir {
            return Notes::Dir(dir);
        }
        let url = self
            .superhub_url
            .clone()
            .or_else(|| std::env::var("SUPERHUB_URL").ok());
        match url {
            Some(url) => Notes::Hub {
                url,
                key: std::env::var("SUPERHUB_API_KEY").ok(),
            },
            None => Notes::Nowhere,
        }
    }

    /// `--out`, else `export_dir`, else `References/strata` in the
    /// SuperHub vault (`superhub_vault`, or `$SUPERHUB_VAULT_PATH`).
    pub fn export_dir(&self, out: Option<PathBuf>) -> anyhow::Result<PathBuf> {
        if let Some(dir) = out.or_else(|| self.export_dir.clone()) {
            return Ok(dir);
        }
        let vault = self
            .superhub_vault
            .clone()
            .or_else(|| std::env::var_os("SUPERHUB_VAULT_PATH").map(PathBuf::from))
            .context("where to? pass --out, or set export_dir or superhub_vault in the config")?;
        Ok(vault.join("References/strata"))
    }

    /// `barbero-cli get <entry>`: its master-password prompt goes to the
    /// terminal, the one password comes back on stdout.
    fn ask_barbero(&self) -> anyhow::Result<String> {
        let out = Command::new(&self.barbero_command)
            .args(["get", &self.barbero_entry])
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit())
            .output()
            .with_context(|| format!("running {}", self.barbero_command))?;
        if !out.status.success() {
            bail!(
                "{} get {} failed ({})",
                self.barbero_command,
                self.barbero_entry,
                out.status
            );
        }
        let text = String::from_utf8(out.stdout).context("Barbero's answer is not UTF-8")?;
        let pass = text.strip_suffix('\n').unwrap_or(&text);
        if pass.is_empty() {
            bail!("Barbero has an empty password for {}", self.barbero_entry);
        }
        Ok(pass.to_string())
    }
}

fn prompt(confirm: bool) -> anyhow::Result<String> {
    // rpassword reads /dev/tty, but with no terminal at all there is
    // nobody to ask: say what would have worked instead
    if !std::io::stderr().is_terminal() && !std::io::stdin().is_terminal() {
        bail!(
            "the vault needs a passphrase: set {PASSPHRASE_ENV}, or cascade = \"barbero\" in the config"
        );
    }
    let pass = rpassword::prompt_password("Vault passphrase: ")?;
    if confirm && rpassword::prompt_password("Again: ")? != pass {
        bail!("the passphrases differ");
    }
    if pass.is_empty() {
        bail!("an empty passphrase");
    }
    Ok(pass)
}
