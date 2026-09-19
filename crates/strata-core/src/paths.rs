//! Where things are by default, the same for every front end.

use std::path::PathBuf;

use crate::{Error, Result};

/// `$STRATA_DIR`, else `$XDG_DATA_HOME/strata`, else
/// `~/.local/share/strata`.
pub fn default_dir() -> Result<PathBuf> {
    if let Some(dir) = env("STRATA_DIR") {
        return Ok(dir);
    }
    Ok(xdg("XDG_DATA_HOME", ".local/share")?.join("strata"))
}

/// `$STRATA_SOCKET`, else `$XDG_RUNTIME_DIR/strata.sock`: strata-server
/// listens there, and the socket's file mode is its access control.
pub fn default_socket() -> Result<PathBuf> {
    if let Some(socket) = env("STRATA_SOCKET") {
        return Ok(socket);
    }
    let runtime = env("XDG_RUNTIME_DIR").ok_or_else(|| {
        Error::Server("neither $STRATA_SOCKET nor $XDG_RUNTIME_DIR is set".into())
    })?;
    Ok(runtime.join("strata.sock"))
}

/// `$XDG_CONFIG_HOME/strata/config.toml`, else under `~/.config`.
pub fn default_config() -> Result<PathBuf> {
    Ok(xdg("XDG_CONFIG_HOME", ".config")?.join("strata/config.toml"))
}

fn env(var: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn xdg(var: &str, fallback: &str) -> Result<PathBuf> {
    match env(var) {
        Some(p) => Ok(p),
        None => {
            let home = env("HOME").ok_or_else(|| {
                Error::Io(std::io::Error::other(format!(
                    "neither $HOME nor ${var} is set"
                )))
            })?;
            Ok(home.join(fallback))
        }
    }
}
