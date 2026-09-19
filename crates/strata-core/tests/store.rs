//! The store's behaviour, run once with the Task type in the open
//! partition and once with it in the vault: the M2 gate says the two must
//! not differ while the vault is unlocked.

use serde_json::json;
use strata_core::{
    Entry, Error, Item, Kind, Partition, PropertyDef, Query, Store, TypeDef, Uuid, Values,
};
use tempfile::TempDir;

/// `name` becomes a module with an `open` and a `vault` test, each
/// calling the function `name` with that partition.
macro_rules! both_partitions {
    ($($name:ident),* $(,)?) => {$(
        mod $name {
            #[test]
            fn open() {
                super::$name(strata_core::Partition::Open)
            }
            #[test]
            fn vault() {
                super::$name(strata_core::Partition::Vault)
            }
        }
    )*};
}

both_partitions!(
    task_type_declared_filled_and_queried,
    query_filters_on_numbers_and_absence,
    query_rejects_unknown_names,
    values_are_checked_against_their_kind,
    update_merges_and_null_removes,
    delete_removes_the_item_and_its_relations,
    relations_need_both_ends,
    properties_change_at_runtime,
    types_are_listed_and_unique,
    a_store_on_disk_survives_reopening,
);

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

fn task_type(partition: Partition) -> TypeDef {
    TypeDef::new("Task")
        .description("something to do")
        .partition(partition)
        .property(PropertyDef::new("title", Kind::Text).required())
        .property(PropertyDef::new("status", Kind::Text).required())
        .property(PropertyDef::new("due", Kind::Date))
        .property(PropertyDef::new("estimate", Kind::Number))
        .property(PropertyDef::new("note", Kind::Text))
}

/// A store on disk with an unlocked vault and nothing in it.
fn empty_store() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let mut store = Store::open(dir.path()).unwrap();
    store.vault_create("correct horse").unwrap();
    (dir, store)
}

fn store_with_tasks(partition: Partition) -> (TempDir, Store) {
    let (dir, store) = empty_store();
    store.add_type(&task_type(partition)).unwrap();
    for (title, status, due, estimate) in [
        ("write M1", "doing", Some("2026-09-20"), Some(3)),
        ("tag M0", "done", Some("2026-09-19"), Some(1)),
        ("plan M2", "todo", Some("2026-09-25"), None),
        ("read SQLCipher docs", "todo", None, Some(2)),
        ("sketch the TUI", "todo", Some("2026-10-01"), Some(5)),
    ] {
        store
            .add_item(
                "Task",
                values(json!({"title": title, "status": status, "due": due, "estimate": estimate})),
                "claude",
            )
            .unwrap();
    }
    (dir, store)
}

fn items(entries: Vec<Entry>) -> Vec<Item> {
    entries
        .into_iter()
        .map(|e| e.into_item().expect("an item, not a placeholder"))
        .collect()
}

fn query(store: &Store, q: Query) -> Vec<Item> {
    items(store.query(&q).unwrap())
}

fn titles(items: &[Item]) -> Vec<&str> {
    items
        .iter()
        .map(|i| i.values["title"].as_str().unwrap())
        .collect()
}

/// The M1 gate: a Task type can be declared, filled and queried.
fn task_type_declared_filled_and_queried(p: Partition) {
    let (_dir, store) = store_with_tasks(p);

    let def = store.get_type("Task").unwrap();
    assert_eq!(def, task_type(p));

    let todo = query(
        &store,
        Query::new("Task").filter("status", "todo").sort_by("due"),
    );
    // no due date sorts last
    assert_eq!(
        titles(&todo),
        ["plan M2", "sketch the TUI", "read SQLCipher docs"]
    );

    let by_estimate = query(
        &store,
        Query::new("Task").sort_by("estimate").descending().limit(2),
    );
    assert_eq!(titles(&by_estimate), ["sketch the TUI", "write M1"]);

    let page = query(&store, Query::new("Task").limit(2).offset(1));
    assert_eq!(titles(&page), ["tag M0", "plan M2"]);

    let first = &todo[0];
    assert_eq!(first.author, "claude");
    assert_eq!(first.type_name, "Task");
    assert!(first.values.get("estimate").is_none(), "null is no value");
}

fn query_filters_on_numbers_and_absence(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    let three = query(&store, Query::new("Task").filter("estimate", 3.0));
    assert_eq!(titles(&three), ["write M1"]);
    let undated = query(
        &store,
        Query::new("Task").filter("due", serde_json::Value::Null),
    );
    assert_eq!(titles(&undated), ["read SQLCipher docs"]);
    let none = query(
        &store,
        Query::new("Task")
            .filter("status", "todo")
            .filter("estimate", 1),
    );
    assert!(none.is_empty());
}

fn query_rejects_unknown_names(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    assert!(matches!(
        store.query(&Query::new("Nope")),
        Err(Error::UnknownType(_))
    ));
    assert!(matches!(
        store.query(&Query::new("Task").filter("colour", "red")),
        Err(Error::UnknownProperty { .. })
    ));
    assert!(matches!(
        store.query(&Query::new("Task").sort_by("colour")),
        Err(Error::UnknownProperty { .. })
    ));
}

fn values_are_checked_against_their_kind(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    let err = store
        .add_item(
            "Task",
            values(json!({"title": "x", "status": "todo", "due": "tomorrow"})),
            "jarda",
        )
        .unwrap_err();
    assert!(matches!(err, Error::InvalidValue { ref property, .. } if property == "due"));

    let err = store
        .add_item("Task", values(json!({"title": "x"})), "jarda")
        .unwrap_err();
    assert!(matches!(err, Error::MissingRequired { ref property } if property == "status"));

    let err = store
        .add_item(
            "Task",
            values(json!({"title": "x", "status": "todo", "colour": "red"})),
            "jarda",
        )
        .unwrap_err();
    assert!(matches!(err, Error::UnknownProperty { .. }));

    let err = store
        .add_item("Task", values(json!({"title": "x", "status": "todo"})), " ")
        .unwrap_err();
    assert!(matches!(err, Error::NoAuthor));
}

fn update_merges_and_null_removes(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    let item = query(&store, Query::new("Task").filter("title", "write M1")).remove(0);

    let updated = store
        .update_item(
            item.id,
            values(json!({"status": "done", "estimate": null})),
            "jarda",
        )
        .unwrap();
    assert_eq!(updated.values["status"], "done");
    assert_eq!(updated.values["title"], "write M1");
    assert!(updated.values.get("estimate").is_none());
    assert_eq!(updated.author, "claude");
    assert_eq!(updated.modified_by, "jarda");
    assert!(updated.modified >= item.modified);
    assert_eq!(updated.created, item.created);

    let err = store
        .update_item(item.id, values(json!({"title": null})), "jarda")
        .unwrap_err();
    assert!(matches!(err, Error::MissingRequired { .. }));
    let err = store
        .update_item(item.id, values(json!({"estimate": "lots"})), "jarda")
        .unwrap_err();
    assert!(matches!(err, Error::InvalidValue { .. }));
    // a refused update leaves the item as it was
    assert_eq!(store.get_item(item.id).unwrap(), updated);
}

fn delete_removes_the_item_and_its_relations(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    let all = query(&store, Query::new("Task"));
    let (a, b) = (all[0].id, all[1].id);
    store.relate(a, b, "blocks").unwrap();
    store.relate(a, b, "blocks").unwrap(); // idempotent
    assert_eq!(store.relations(b).unwrap().len(), 1);

    store.delete_item(a).unwrap();
    assert!(matches!(store.get_item(a), Err(Error::UnknownItem(_))));
    assert!(store.relations(b).unwrap().is_empty());
    assert!(matches!(store.delete_item(a), Err(Error::UnknownItem(_))));
    assert_eq!(store.query(&Query::new("Task")).unwrap().len(), 4);
}

fn relations_need_both_ends(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    let all = query(&store, Query::new("Task"));
    let (a, b) = (all[0].id, all[1].id);
    store.relate(a, b, "about").unwrap();
    let rels = store.relations(a).unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!((rels[0].source, rels[0].target), (a, b));
    assert_eq!(store.get(rels[0].target).unwrap().id(), b);
    store.unrelate(a, b, "about").unwrap();
    assert!(store.relations(a).unwrap().is_empty());
    assert!(matches!(
        store.relate(Uuid::now_v7(), a, "about"),
        Err(Error::UnknownItem(_))
    ));
    assert!(matches!(
        store.relate(a, Uuid::now_v7(), "about"),
        Err(Error::UnknownItem(_))
    ));
}

fn properties_change_at_runtime(p: Partition) {
    let (_dir, store) = store_with_tasks(p);

    store
        .add_property("Task", &PropertyDef::new("project", Kind::Text))
        .unwrap();
    assert!(matches!(
        store.add_property("Task", &PropertyDef::new("owner", Kind::Text).required()),
        Err(Error::RequiredUnmet { count: 5, .. })
    ));
    assert!(matches!(
        store.add_property("Task", &PropertyDef::new("due", Kind::Date)),
        Err(Error::PropertyExists { .. })
    ));
    assert!(matches!(
        store.add_property("Task", &PropertyDef::new("modified", Kind::Date)),
        Err(Error::ReservedName(_))
    ));

    store.rename_property("Task", "estimate", "hours").unwrap();
    let def = store.get_type("Task").unwrap();
    let names: Vec<_> = def.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["title", "status", "due", "hours", "note", "project"]
    );
    let three = query(&store, Query::new("Task").filter("hours", 3));
    assert_eq!(titles(&three), ["write M1"]);

    assert!(matches!(
        store.set_required("Task", "hours", true),
        Err(Error::RequiredUnmet { count: 1, .. })
    ));
    store.set_required("Task", "title", false).unwrap();
    store.set_required("Task", "title", true).unwrap();

    store.remove_property("Task", "hours").unwrap();
    let item = &query(&store, Query::new("Task"))[0];
    assert!(item.values.get("hours").is_none());
    assert!(store.get_type("Task").unwrap().get("hours").is_none());
}

fn types_are_listed_and_unique(p: Partition) {
    let (_dir, store) = store_with_tasks(p);
    store
        .add_type(
            &TypeDef::new("Bookmark")
                .partition(p)
                .property(PropertyDef::new("url", Kind::Text)),
        )
        .unwrap();
    let names: Vec<_> = store
        .list_types()
        .unwrap()
        .into_iter()
        .map(|t| t.name)
        .collect();
    assert_eq!(names, ["Bookmark", "Task"]);
    assert!(matches!(
        store.add_type(&TypeDef::new("Task")),
        Err(Error::TypeExists(_))
    ));
    assert!(matches!(
        store.add_type(&TypeDef::new("two words")),
        Err(Error::InvalidName(_))
    ));
    let dup = TypeDef::new("Dup")
        .partition(p)
        .property(PropertyDef::new("a", Kind::Text))
        .property(PropertyDef::new("a", Kind::Number));
    assert!(matches!(
        store.add_type(&dup),
        Err(Error::PropertyExists { .. })
    ));
    assert!(matches!(store.get_type("Dup"), Err(Error::UnknownType(_))));
    store
        .set_description("Bookmark", "a link worth keeping")
        .unwrap();
    assert_eq!(
        store.get_type("Bookmark").unwrap().description,
        "a link worth keeping"
    );
}

fn a_store_on_disk_survives_reopening(p: Partition) {
    let (dir, store) = store_with_tasks(p);
    let id = store
        .add_item(
            "Task",
            values(json!({"title": "persist", "status": "todo"})),
            "jarda",
        )
        .unwrap()
        .id;
    drop(store);
    let mut store = Store::open(dir.path()).unwrap();
    if p == Partition::Vault {
        assert!(matches!(store.get_item(id), Err(Error::VaultLocked)));
        store.vault_unlock("correct horse").unwrap();
    }
    assert_eq!(store.get_item(id).unwrap().values["title"], "persist");
    assert_eq!(store.query(&Query::new("Task")).unwrap().len(), 6);
}
