//! Opening a SQLCipher database. SQLCipher derives the key from the
//! passphrase itself (PBKDF2 with its default parameters); strata adds no
//! KDF of its own.

use std::path::Path;

use rusqlite::{Connection, ErrorCode};

use crate::{Error, Result};

/// Open (or create) the encrypted database at `path` with `passphrase`.
///
/// `PRAGMA key` never fails by itself: a wrong key only shows on the
/// first read. So the key is checked here, by reading `sqlite_master`,
/// and a mismatch comes back as [`Error::WrongPassphrase`] rather than
/// as a later, puzzling "file is not a database".
pub fn open_encrypted(path: &Path, passphrase: &str) -> Result<Connection> {
    let conn = Connection::open(path)?;
    // A wrong key is an answer here, not a fault: keep SQLCipher from
    // printing its decrypt errors to the caller's stderr.
    conn.pragma_update(None, "cipher_log_level", "NONE")?;
    conn.pragma_update(None, "key", passphrase)?;
    match conn.query_row("SELECT count(*) FROM sqlite_master", [], |_| Ok(())) {
        Ok(()) => Ok(conn),
        Err(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::NotADatabase => {
            Err(Error::WrongPassphrase(path.to_path_buf()))
        }
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_needs_the_right_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vault.db");

        let conn = open_encrypted(&path, "correct horse").unwrap();
        conn.execute_batch("CREATE TABLE t (v TEXT); INSERT INTO t VALUES ('only with the key');")
            .unwrap();
        drop(conn);

        // The file on disk is not plain SQLite, and the value is not in it.
        let raw = std::fs::read(&path).unwrap();
        assert!(!raw.starts_with(b"SQLite format 3"));
        assert!(!raw.windows(17).any(|w| w == b"only with the key"));

        let err = open_encrypted(&path, "wrong horse").unwrap_err();
        assert!(matches!(err, Error::WrongPassphrase(_)), "{err:?}");

        let conn = open_encrypted(&path, "correct horse").unwrap();
        let v: String = conn.query_row("SELECT v FROM t", [], |r| r.get(0)).unwrap();
        assert_eq!(v, "only with the key");
    }
}
