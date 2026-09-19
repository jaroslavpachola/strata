//! How a front end finds its store: the server, when one is running for
//! it, else the files.

use std::path::{Path, PathBuf};

use strata_core::{Api, Error, Result, Store, paths};

use crate::Client;

/// The server a front end for `dir` should talk to, if any.
/// `$STRATA_SOCKET` names one to use whatever the directory, and it must
/// answer; the default socket counts only if its server holds `dir`, so a
/// script with a store of its own gets that store.
pub fn server_for(dir: &Path) -> Result<Option<Client>> {
    if let Some(socket) = std::env::var_os("STRATA_SOCKET").filter(|s| !s.is_empty()) {
        return Client::connect(Path::new(&socket)).map(Some);
    }
    if let Ok(socket) = paths::default_socket()
        && let Ok(client) = Client::connect(&socket)
        && same_dir(&client.info().dir, dir)
    {
        return Ok(Some(client));
    }
    Ok(None)
}

/// The store in `dir`, through its server if one is running. With no
/// server and no store, an error, unless `create` says to make one.
pub fn connect(dir: &Path, create: bool) -> Result<Box<dyn Api>> {
    if let Some(client) = server_for(dir)? {
        return Ok(Box::new(client));
    }
    if !create && !dir.join("open.db").exists() {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no store at {}: run `strata init`", dir.display()),
        )));
    }
    Ok(Box::new(Store::open(dir)?))
}

/// The socket `strata watch` and the TUI listen on: `$STRATA_SOCKET`, or
/// the default.
pub fn socket() -> Result<PathBuf> {
    paths::default_socket()
}

fn same_dir(a: &Path, b: &Path) -> bool {
    let canon = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canon(a) == canon(b)
}
