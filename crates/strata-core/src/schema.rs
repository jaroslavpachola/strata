//! The meta-schema and its migrations.
//!
//! Each database, open and vault alike, carries the same tables. The
//! vault is attached to the open database's connection, so a migration
//! names its tables `{db}.name` and runs once per schema, `main` and
//! `vault`. `PRAGMA user_version` counts the migrations a database has
//! seen; the ones it has not run in one transaction.

use rusqlite::Connection;

use crate::Result;

/// Append only. A shipped migration is never edited, only followed.
const MIGRATIONS: &[&str] = &[
    // 1: the meta-schema
    "
    CREATE TABLE {db}.type (
        name        TEXT PRIMARY KEY,
        partition   TEXT NOT NULL DEFAULT 'open' CHECK (partition IN ('open', 'vault')),
        description TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE {db}.property (
        type     TEXT NOT NULL REFERENCES type(name) ON DELETE CASCADE ON UPDATE CASCADE,
        name     TEXT NOT NULL,
        kind     TEXT NOT NULL CHECK (kind IN ('text', 'number', 'date', 'bool', 'json', 'ref')),
        required INTEGER NOT NULL DEFAULT 0,
        position INTEGER NOT NULL,
        PRIMARY KEY (type, name)
    );
    CREATE TABLE {db}.item (
        id          TEXT PRIMARY KEY,
        type        TEXT NOT NULL REFERENCES type(name) ON UPDATE CASCADE,
        created     TEXT NOT NULL,
        modified    TEXT NOT NULL,
        author      TEXT NOT NULL,
        modified_by TEXT NOT NULL
    );
    CREATE INDEX {db}.item_type ON item(type);
    CREATE TABLE {db}.value (
        item     TEXT NOT NULL REFERENCES item(id) ON DELETE CASCADE,
        property TEXT NOT NULL,
        value    TEXT NOT NULL,
        PRIMARY KEY (item, property)
    );
    CREATE INDEX {db}.value_property ON value(property);
    -- no foreign keys: either end may live in the other database
    CREATE TABLE {db}.relation (
        source TEXT NOT NULL,
        target TEXT NOT NULL,
        kind   TEXT NOT NULL,
        PRIMARY KEY (source, target, kind)
    );
    CREATE INDEX {db}.relation_target ON relation(target);
    ",
    // 2: what the open database may know of the vault's items while it
    // is locked: that an id exists and its type, nothing more. Unused in
    // the vault itself.
    "
    CREATE TABLE {db}.vault_index (
        id   TEXT PRIMARY KEY,
        type TEXT NOT NULL REFERENCES type(name) ON UPDATE CASCADE
    );
    CREATE INDEX {db}.vault_index_type ON vault_index(type);
    ",
];

/// Bring schema `db` (`main` or `vault`) of `conn` up to date.
pub(crate) fn migrate(conn: &Connection, db: &str) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let applied: i64 = tx.query_row(&format!("PRAGMA {db}.user_version"), [], |r| r.get(0))?;
    for sql in MIGRATIONS.iter().skip(applied as usize) {
        tx.execute_batch(&sql.replace("{db}", db))?;
    }
    tx.execute_batch(&format!("PRAGMA {db}.user_version = {}", MIGRATIONS.len()))?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrating_twice_is_a_no_op() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn, "main").unwrap();
        migrate(&conn, "main").unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }

    #[test]
    fn an_attached_schema_gets_its_own_tables() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("ATTACH ':memory:' AS vault").unwrap();
        migrate(&conn, "main").unwrap();
        migrate(&conn, "vault").unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT count(*) FROM vault.sqlite_master WHERE type = 'table'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 6);
    }
}
