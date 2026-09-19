//! The meta-schema and its migrations.
//!
//! Each database, open and vault alike, carries the same five tables.
//! `PRAGMA user_version` counts the migrations applied; opening a
//! database runs the ones it has not seen, all in one transaction.

use rusqlite::Connection;

use crate::Result;

/// Append only. A shipped migration is never edited, only followed.
const MIGRATIONS: &[&str] = &[
    // 1: the meta-schema
    "
    CREATE TABLE type (
        name        TEXT PRIMARY KEY,
        partition   TEXT NOT NULL DEFAULT 'open' CHECK (partition IN ('open', 'vault')),
        description TEXT NOT NULL DEFAULT ''
    );
    CREATE TABLE property (
        type     TEXT NOT NULL REFERENCES type(name) ON DELETE CASCADE ON UPDATE CASCADE,
        name     TEXT NOT NULL,
        kind     TEXT NOT NULL CHECK (kind IN ('text', 'number', 'date', 'bool', 'json', 'ref')),
        required INTEGER NOT NULL DEFAULT 0,
        position INTEGER NOT NULL,
        PRIMARY KEY (type, name)
    );
    CREATE TABLE item (
        id          TEXT PRIMARY KEY,
        type        TEXT NOT NULL REFERENCES type(name) ON UPDATE CASCADE,
        created     TEXT NOT NULL,
        modified    TEXT NOT NULL,
        author      TEXT NOT NULL,
        modified_by TEXT NOT NULL
    );
    CREATE INDEX item_type ON item(type);
    CREATE TABLE value (
        item     TEXT NOT NULL REFERENCES item(id) ON DELETE CASCADE,
        property TEXT NOT NULL,
        value    TEXT NOT NULL,
        PRIMARY KEY (item, property)
    );
    CREATE INDEX value_property ON value(property);
    -- no foreign keys: either end may live in the other database
    CREATE TABLE relation (
        source TEXT NOT NULL,
        target TEXT NOT NULL,
        kind   TEXT NOT NULL,
        PRIMARY KEY (source, target, kind)
    );
    CREATE INDEX relation_target ON relation(target);
    ",
];

/// Bring `conn` up to the latest schema.
pub(crate) fn migrate(conn: &mut Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    let tx = conn.transaction()?;
    let applied: i64 = tx.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for sql in MIGRATIONS.iter().skip(applied as usize) {
        tx.execute_batch(sql)?;
    }
    tx.pragma_update(None, "user_version", MIGRATIONS.len() as i64)?;
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrating_twice_is_a_no_op() {
        let mut conn = Connection::open_in_memory().unwrap();
        migrate(&mut conn).unwrap();
        migrate(&mut conn).unwrap();
        let v: i64 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
    }
}
