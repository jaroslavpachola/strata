//! Export and import: what goes out comes back the same.

use serde_json::json;
use strata_core::{Error, Kind, Partition, PropertyDef, Query, Store, TypeDef, TypeExport, Values};
use tempfile::TempDir;

const PASS: &str = "transfer";

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

fn store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.vault_create(PASS).unwrap();
    (dir, store)
}

/// Tasks in the open, accounts in the vault, relations both ways.
fn filled() -> (TempDir, Store) {
    let (dir, store) = store();
    store.seed().unwrap();
    store
        .add_type(
            &TypeDef::new("Account")
                .partition(Partition::Vault)
                .property(PropertyDef::new("name", Kind::Text).required()),
        )
        .unwrap();
    let account = store
        .add_item("Account", values(json!({"name": "savings"})), "jarda")
        .unwrap();
    let tasks = store
        .add_items(
            "Task",
            vec![
                values(json!({"title": "a", "status": "todo", "due": "2026-10-01"})),
                values(json!({"title": "b", "status": "done"})),
            ],
            "claude",
        )
        .unwrap();
    store
        .update_item(tasks[1].id, values(json!({"project": "strata"})), "jarda")
        .unwrap();
    store.relate(tasks[0].id, account.id, "about").unwrap();
    store.relate(account.id, tasks[1].id, "reminder").unwrap();
    store.relate(tasks[0].id, tasks[1].id, "before").unwrap();
    (dir, store)
}

fn everything(store: &Store) -> Vec<TypeExport> {
    store
        .list_types()
        .unwrap()
        .iter()
        .map(|t| store.export_type(&t.name).unwrap())
        .collect()
}

#[test]
fn an_export_imported_into_an_empty_store_is_the_same_store() {
    let (_a, source) = filled();
    let exported = everything(&source);

    let (_b, target) = store();
    let imported = target.import(&exported).unwrap();
    assert_eq!(imported.types, ["Account", "PortfolioSnapshot", "Task"]);
    assert_eq!((imported.items, imported.relations), (3, 3));
    assert_eq!(everything(&target), exported);

    // ids, times and authors as they were, and the vault still the vault
    let task = &exported
        .iter()
        .find(|t| t.def.name == "Task")
        .unwrap()
        .items[1];
    let back = target.get_item(task.id).unwrap();
    assert_eq!(
        (back.author.as_str(), back.modified_by.as_str()),
        ("claude", "jarda")
    );
    assert_eq!(back.modified, task.modified);
    let mut target = target;
    target.vault_lock().unwrap();
    assert!(
        target
            .query(&Query::new("Account"))
            .unwrap()
            .iter()
            .all(|e| e.item().is_none())
    );
}

#[test]
fn seed_types_that_match_are_reused() {
    let (_a, source) = filled();
    let exported = everything(&source);
    let (_b, target) = store();
    target.seed().unwrap();
    let imported = target.import(&exported).unwrap();
    assert_eq!(imported.types, ["Account"], "Task and the snapshot matched");
}

#[test]
fn an_edited_definition_replaces_an_empty_type_and_not_a_full_one() {
    let (_a, source) = filled();
    let mut exported = everything(&source);

    // the schema change the round trip is for: rename a property
    let tasks = exported.iter_mut().find(|t| t.def.name == "Task").unwrap();
    tasks.def.properties[3].name = "area".into();
    for item in &mut tasks.items {
        if let Some(v) = item.values.remove("project") {
            item.values.insert("area".into(), v);
        }
    }

    let (_b, target) = store();
    target.seed().unwrap(); // an empty Task, the old shape
    target.import(&exported).unwrap();
    let def = target.get_type("Task").unwrap();
    assert!(def.get("area").is_some() && def.get("project").is_none());
    let b = target
        .query(&Query::new("Task").filter("area", "strata"))
        .unwrap();
    assert_eq!(b.len(), 1);

    // the same edit against a store whose Task has items is refused
    let (_c, full) = filled();
    let mut into_full = exported.clone();
    into_full.retain(|t| t.def.name == "Task");
    for item in &mut into_full[0].items {
        item.id = strata_core::Uuid::now_v7();
    }
    into_full[0].relations.clear();
    assert!(matches!(full.import(&into_full), Err(Error::TypeConflict(t)) if t == "Task"));
}

#[test]
fn an_import_is_all_or_nothing() {
    let (_a, source) = filled();
    let mut exported = everything(&source);
    let tasks = exported.iter_mut().find(|t| t.def.name == "Task").unwrap();
    tasks.items[1]
        .values
        .insert("status".into(), json!("someday"));

    let (_b, target) = store();
    assert!(matches!(
        target.import(&exported),
        Err(Error::NotAChoice { .. })
    ));
    assert!(target.list_types().unwrap().is_empty());

    // importing the same thing twice: the ids are taken
    let exported = everything(&source);
    target.import(&exported).unwrap();
    assert!(matches!(
        target.import(&exported),
        Err(Error::ItemExists(_))
    ));
}

#[test]
fn a_locked_vault_neither_exports_nor_imports() {
    let (_a, mut source) = filled();
    let account = source.export_type("Account").unwrap();
    source.vault_lock().unwrap();
    assert!(matches!(
        source.export_type("Account"),
        Err(Error::VaultLocked)
    ));

    let (_b, mut target) = store();
    target.vault_lock().unwrap();
    let mut only = account;
    only.relations.clear();
    assert!(matches!(target.import(&[only]), Err(Error::VaultLocked)));
}
