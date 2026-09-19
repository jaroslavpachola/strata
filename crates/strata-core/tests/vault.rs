//! The vault: its passphrase, what a locked store shows, and moving
//! types between the partitions.

use serde_json::json;
use strata_core::{
    Entry, Error, Kind, Partition, PropertyDef, Query, Store, TypeDef, Uuid, Values, VaultStatus,
};
use tempfile::TempDir;

const PASS: &str = "correct horse";
/// Written into vault items; must never show while the vault is locked.
const SECRET: &str = "IBAN CZ65 0800 0000 1920 0014 5399";

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

fn account_type() -> TypeDef {
    TypeDef::new("Account")
        .partition(Partition::Vault)
        .property(PropertyDef::new("name", Kind::Text).required())
        .property(PropertyDef::new("number", Kind::Text))
        .property(PropertyDef::new("balance", Kind::Number))
}

fn task_type() -> TypeDef {
    TypeDef::new("Task")
        .property(PropertyDef::new("title", Kind::Text).required())
        .property(PropertyDef::new("account", Kind::Ref))
}

struct Fixture {
    /// Bind it, even as `_dir`: dropped, it deletes the store under us.
    dir: TempDir,
    store: Store,
    account: Uuid,
    task: Uuid,
}

/// An account in the vault and a task in the open that points at it,
/// both ways: a relation from each, and the task's `account` ref.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.vault_create(PASS).unwrap();
    store.add_type(&account_type()).unwrap();
    store.add_type(&task_type()).unwrap();
    let account = store
        .add_item(
            "Account",
            values(json!({"name": "rainy day fund", "number": SECRET, "balance": 1234.5})),
            "jarda",
        )
        .unwrap()
        .id;
    let task = store
        .add_item(
            "Task",
            values(json!({"title": "check the savings", "account": account.to_string()})),
            "claude",
        )
        .unwrap()
        .id;
    store.relate(task, account, "about").unwrap();
    store.relate(account, task, "reminder").unwrap();
    Fixture {
        dir,
        store,
        account,
        task,
    }
}

fn file_contains(path: &std::path::Path, needle: &str) -> bool {
    let bytes = std::fs::read(path).unwrap();
    bytes.windows(needle.len()).any(|w| w == needle.as_bytes())
}

#[test]
fn status_follows_create_lock_and_unlock() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(store.vault_status(), VaultStatus::Absent);
    assert!(matches!(store.vault_unlock(PASS), Err(Error::NoVault)));
    assert!(matches!(
        store.add_type(&account_type()),
        Err(Error::NoVault)
    ));

    store.vault_create(PASS).unwrap();
    assert_eq!(store.vault_status(), VaultStatus::Unlocked);
    assert!(matches!(store.vault_create(PASS), Err(Error::VaultExists)));
    store.vault_unlock(PASS).unwrap(); // already open: a no-op

    store.vault_lock().unwrap();
    store.vault_lock().unwrap();
    assert_eq!(store.vault_status(), VaultStatus::Locked);
    assert!(matches!(store.vault_create(PASS), Err(Error::VaultExists)));
}

#[test]
fn a_wrong_passphrase_is_an_error_and_leaves_it_locked() {
    let Fixture {
        dir,
        mut store,
        account,
        ..
    } = fixture();
    store.vault_lock().unwrap();

    let err = store.vault_unlock("wrong horse").unwrap_err();
    assert!(matches!(err, Error::WrongPassphrase(_)), "{err:?}");
    assert_eq!(store.vault_status(), VaultStatus::Locked);
    assert!(matches!(store.get_item(account), Err(Error::VaultLocked)));

    // the store is still usable, and the right passphrase still works
    store.vault_unlock(PASS).unwrap();
    assert_eq!(store.get_item(account).unwrap().values["number"], SECRET);

    // and so does a fresh process
    drop(store);
    let mut store = Store::open(dir.path()).unwrap();
    assert_eq!(store.vault_status(), VaultStatus::Locked);
    store.vault_unlock(PASS).unwrap();
    assert_eq!(store.get_item(account).unwrap().values["balance"], 1234.5);
}

/// The M2 gate, second half: a locked store never leaks a vault value.
#[test]
fn a_locked_store_never_leaks_a_vault_value() {
    let Fixture {
        dir,
        mut store,
        account,
        task,
    } = fixture();
    store.vault_lock().unwrap();

    // Everything a reader can get out of the locked store, as JSON.
    let mut seen = Vec::new();
    let mut show = |v: serde_json::Value| seen.push(v.to_string());

    show(json!(store.list_types().unwrap()));
    show(json!(store.get_type("Account").unwrap()));
    let accounts = store.query(&Query::new("Account")).unwrap();
    show(json!(accounts));
    show(json!(store.query(&Query::new("Task")).unwrap()));
    show(json!(store.get(account).unwrap()));
    show(json!(store.get(task).unwrap()));
    show(json!(store.relations(task).unwrap()));
    show(json!(store.relations(account).unwrap()));

    let joined = seen.join("\n");
    for leak in [SECRET, "rainy day fund", "1234.5", "reminder"] {
        assert!(!joined.contains(leak), "{leak:?} leaked: {joined}");
    }

    // placeholders: the id and the type, nothing else
    assert_eq!(accounts.len(), 1);
    let Entry::Locked(ref locked) = accounts[0] else {
        panic!("expected a placeholder, got {:?}", accounts[0]);
    };
    assert_eq!((locked.id, locked.type_name.as_str()), (account, "Account"));
    assert_eq!(
        serde_json::to_value(&accounts[0]).unwrap(),
        json!({"id": account, "type": "Account", "locked": true})
    );

    // the open side's relation to the vault is visible, the vault's own is not
    let rels = store.relations(task).unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!((rels[0].target, rels[0].kind.as_str()), (account, "about"));

    // anything that would need the values is refused, not answered
    assert!(matches!(store.get_item(account), Err(Error::VaultLocked)));
    assert!(matches!(
        store.query(&Query::new("Account").filter("name", "rainy day fund")),
        Err(Error::VaultLocked)
    ));
    assert!(matches!(
        store.query(&Query::new("Account").sort_by("balance")),
        Err(Error::VaultLocked)
    ));
    assert!(matches!(
        store.query(&Query::new("Account").sort_by("modified")),
        Err(Error::VaultLocked)
    ));

    // and neither file holds the secret in the clear
    assert!(!file_contains(&dir.path().join("open.db"), SECRET));
    assert!(!file_contains(&dir.path().join("vault.db"), SECRET));
}

#[test]
fn a_locked_vault_refuses_writes() {
    let Fixture {
        mut store,
        account,
        task,
        dir: _dir,
        ..
    } = fixture();
    store.vault_lock().unwrap();

    let locked = |r: Result<_, Error>| assert!(matches!(r, Err(Error::VaultLocked)));
    locked(
        store
            .add_item("Account", values(json!({"name": "x"})), "jarda")
            .map(drop),
    );
    locked(
        store
            .update_item(account, values(json!({"name": "x"})), "jarda")
            .map(drop),
    );
    locked(store.delete_item(account));
    locked(store.relate(account, task, "about"));
    locked(store.unrelate(account, task, "reminder"));
    locked(store.add_property("Account", &PropertyDef::new("iban", Kind::Text)));
    locked(store.remove_property("Account", "number"));
    locked(store.rename_property("Account", "number", "iban"));
    locked(store.set_required("Account", "number", true));
    locked(store.set_description("Account", "money"));
    locked(store.move_type("Account", Partition::Open));
    locked(store.move_type("Task", Partition::Vault));
    locked(
        store.add_type(
            &TypeDef::new("Secret")
                .partition(Partition::Vault)
                .property(PropertyDef::new("v", Kind::Text)),
        ),
    );

    // the open partition carries on
    store
        .update_item(task, values(json!({"title": "still open"})), "jarda")
        .unwrap();

    store.vault_unlock(PASS).unwrap();
    let item = store.get_item(account).unwrap();
    assert_eq!(item.values["number"], SECRET);
    assert_eq!(store.relations(account).unwrap().len(), 2);
}

#[test]
fn a_locked_type_still_pages_in_creation_order() {
    let Fixture {
        mut store,
        dir: _dir,
        ..
    } = fixture();
    let mut ids = vec![store.query(&Query::new("Account")).unwrap()[0].id()];
    for n in 0..4 {
        let item = store
            .add_item("Account", values(json!({"name": format!("a{n}")})), "jarda")
            .unwrap();
        ids.push(item.id);
    }
    store.vault_lock().unwrap();
    let page: Vec<_> = store
        .query(&Query::new("Account").limit(2).offset(1))
        .unwrap()
        .iter()
        .map(Entry::id)
        .collect();
    assert_eq!(page, ids[1..3]);
    let last: Vec<_> = store
        .query(&Query::new("Account").descending().limit(1))
        .unwrap()
        .iter()
        .map(Entry::id)
        .collect();
    assert_eq!(last, ids[4..]);
}

#[test]
fn moving_a_type_carries_its_items_both_ways() {
    let Fixture {
        dir,
        mut store,
        account,
        task,
    } = fixture();
    let before = store.get_item(task).unwrap();

    store.move_type("Task", Partition::Vault).unwrap();
    assert_eq!(store.get_type("Task").unwrap().partition, Partition::Vault);
    assert_eq!(store.get_item(task).unwrap(), before);
    assert_eq!(store.relations(task).unwrap().len(), 2);

    // now vault items: the title leaves open.db, pages and all
    assert!(!file_contains(
        &dir.path().join("open.db"),
        "check the savings"
    ));
    store.vault_lock().unwrap();
    assert!(matches!(store.get(task).unwrap(), Entry::Locked(_)));
    assert!(store.relations(task).unwrap().is_empty());
    store.vault_unlock(PASS).unwrap();

    store.move_type("Account", Partition::Open).unwrap();
    store.move_type("Task", Partition::Open).unwrap();
    store.vault_lock().unwrap();
    assert_eq!(store.get_item(task).unwrap(), before);
    assert_eq!(store.get_item(account).unwrap().values["number"], SECRET);
    assert_eq!(store.relations(task).unwrap().len(), 2);
    assert!(
        store
            .query(&Query::new("Account"))
            .unwrap()
            .iter()
            .all(|e| e.item().is_some())
    );

    // moving to where a type already is changes nothing
    store.move_type("Task", Partition::Open).unwrap();
}

#[test]
fn a_vault_relation_to_an_item_deleted_while_locked_goes_at_unlock() {
    let Fixture {
        mut store,
        account,
        task,
        dir: _dir,
        ..
    } = fixture();
    store.vault_lock().unwrap();
    store.delete_item(task).unwrap();
    store.vault_unlock(PASS).unwrap();
    assert!(store.relations(account).unwrap().is_empty());
}

#[test]
fn deleting_a_vault_item_clears_open_relations_to_it() {
    let Fixture {
        store,
        account,
        task,
        dir: _dir,
        ..
    } = fixture();
    store.delete_item(account).unwrap();
    assert!(store.relations(task).unwrap().is_empty());
    assert!(matches!(store.get(account), Err(Error::UnknownItem(_))));
}

#[test]
fn an_in_memory_vault_is_gone_once_locked() {
    let mut store = Store::open_in_memory().unwrap();
    store.vault_create(PASS).unwrap();
    store.add_type(&account_type()).unwrap();
    store
        .add_item("Account", values(json!({"name": "x"})), "jarda")
        .unwrap();
    store.vault_lock().unwrap();
    assert_eq!(store.vault_status(), VaultStatus::Absent);
}
