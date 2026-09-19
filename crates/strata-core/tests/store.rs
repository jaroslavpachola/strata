use serde_json::json;
use strata_core::{Error, Kind, PropertyDef, Query, Store, TypeDef, Uuid, Values};

fn values(v: serde_json::Value) -> Values {
    v.as_object().unwrap().clone()
}

fn task_type() -> TypeDef {
    TypeDef::new("Task")
        .description("something to do")
        .property(PropertyDef::new("title", Kind::Text).required())
        .property(PropertyDef::new("status", Kind::Text).required())
        .property(PropertyDef::new("due", Kind::Date))
        .property(PropertyDef::new("estimate", Kind::Number))
        .property(PropertyDef::new("note", Kind::Text))
}

fn store_with_tasks() -> Store {
    let store = Store::open_in_memory().unwrap();
    store.add_type(&task_type()).unwrap();
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
    store
}

fn titles(items: &[strata_core::Item]) -> Vec<&str> {
    items
        .iter()
        .map(|i| i.values["title"].as_str().unwrap())
        .collect()
}

/// The M1 gate: a Task type can be declared, filled and queried.
#[test]
fn task_type_declared_filled_and_queried() {
    let store = store_with_tasks();

    let def = store.get_type("Task").unwrap();
    assert_eq!(def, task_type());

    let todo = store
        .query(&Query::new("Task").filter("status", "todo").sort_by("due"))
        .unwrap();
    // no due date sorts last
    assert_eq!(
        titles(&todo),
        ["plan M2", "sketch the TUI", "read SQLCipher docs"]
    );

    let by_estimate = store
        .query(&Query::new("Task").sort_by("estimate").descending().limit(2))
        .unwrap();
    assert_eq!(titles(&by_estimate), ["sketch the TUI", "write M1"]);

    let page = store.query(&Query::new("Task").limit(2).offset(1)).unwrap();
    assert_eq!(titles(&page), ["tag M0", "plan M2"]);

    let first = &todo[0];
    assert_eq!(first.author, "claude");
    assert_eq!(first.type_name, "Task");
    assert!(first.values.get("estimate").is_none(), "null is no value");
}

#[test]
fn query_filters_on_numbers_and_absence() {
    let store = store_with_tasks();
    let three = store
        .query(&Query::new("Task").filter("estimate", 3.0))
        .unwrap();
    assert_eq!(titles(&three), ["write M1"]);
    let undated = store
        .query(&Query::new("Task").filter("due", serde_json::Value::Null))
        .unwrap();
    assert_eq!(titles(&undated), ["read SQLCipher docs"]);
    let none = store
        .query(
            &Query::new("Task")
                .filter("status", "todo")
                .filter("estimate", 1),
        )
        .unwrap();
    assert!(none.is_empty());
}

#[test]
fn query_rejects_unknown_names() {
    let store = store_with_tasks();
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

#[test]
fn values_are_checked_against_their_kind() {
    let store = store_with_tasks();
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

#[test]
fn update_merges_and_null_removes() {
    let store = store_with_tasks();
    let item = store
        .query(&Query::new("Task").filter("title", "write M1"))
        .unwrap()
        .remove(0);

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

#[test]
fn delete_removes_the_item_and_its_relations() {
    let store = store_with_tasks();
    let all = store.query(&Query::new("Task")).unwrap();
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

#[test]
fn relations_accept_a_target_the_store_cannot_see() {
    let store = store_with_tasks();
    let a = store.query(&Query::new("Task")).unwrap()[0].id;
    let elsewhere = Uuid::now_v7();
    store.relate(a, elsewhere, "about").unwrap();
    let rels = store.relations(a).unwrap();
    assert_eq!(rels.len(), 1);
    assert_eq!(rels[0].target, elsewhere);
    store.unrelate(a, elsewhere, "about").unwrap();
    assert!(store.relations(a).unwrap().is_empty());
    assert!(matches!(
        store.relate(Uuid::now_v7(), a, "about"),
        Err(Error::UnknownItem(_))
    ));
}

#[test]
fn properties_change_at_runtime() {
    let store = store_with_tasks();

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
    let three = store.query(&Query::new("Task").filter("hours", 3)).unwrap();
    assert_eq!(titles(&three), ["write M1"]);

    assert!(matches!(
        store.set_required("Task", "hours", true),
        Err(Error::RequiredUnmet { count: 1, .. })
    ));
    store.set_required("Task", "title", false).unwrap();
    store.set_required("Task", "title", true).unwrap();

    store.remove_property("Task", "hours").unwrap();
    let item = &store.query(&Query::new("Task")).unwrap()[0];
    assert!(item.values.get("hours").is_none());
    assert!(store.get_type("Task").unwrap().get("hours").is_none());
}

#[test]
fn types_are_listed_and_unique() {
    let store = store_with_tasks();
    store
        .add_type(&TypeDef::new("Bookmark").property(PropertyDef::new("url", Kind::Text)))
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

#[test]
fn a_store_on_disk_survives_reopening() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let store = Store::open(dir.path()).unwrap();
        store.add_type(&task_type()).unwrap();
        store
            .add_item(
                "Task",
                values(json!({"title": "persist", "status": "todo"})),
                "jarda",
            )
            .unwrap()
            .id
    };
    assert!(dir.path().join("open.db").exists());
    let store = Store::open(dir.path()).unwrap();
    assert_eq!(store.get_item(id).unwrap().values["title"], "persist");
}
